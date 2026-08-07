use std::{collections::HashMap, marker::PhantomData};

use che_orm::{
    FieldInfo, FieldType, Model, ModelField, NaiveDateTime, QueryBuilder, SqliteModel, SqliteValue,
};

use crate::error::AppResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lookup {
    Exact,
    Contains,
    Gt,
    Gte,
    Lt,
    Lte,
}

#[derive(Debug, Clone, Copy)]
pub struct Filter<M = ()> {
    pub name: &'static str,
    pub source: &'static str,
    pub lookup: Lookup,
    _model: PhantomData<fn() -> M>,
}

impl<M> Filter<M> {
    pub const fn exact(field: ModelField<M>) -> Self {
        Self::new(field.db_name(), field.db_name(), Lookup::Exact)
    }

    pub const fn contains(field: ModelField<M>) -> Self {
        Self::new(field.db_name(), field.db_name(), Lookup::Contains)
    }

    pub const fn gt(field: ModelField<M>) -> Self {
        Self::new(field.db_name(), field.db_name(), Lookup::Gt)
    }

    pub const fn gte(field: ModelField<M>) -> Self {
        Self::new(field.db_name(), field.db_name(), Lookup::Gte)
    }

    pub const fn lt(field: ModelField<M>) -> Self {
        Self::new(field.db_name(), field.db_name(), Lookup::Lt)
    }

    pub const fn lte(field: ModelField<M>) -> Self {
        Self::new(field.db_name(), field.db_name(), Lookup::Lte)
    }

    pub const fn exact_source(name: &'static str, field: ModelField<M>) -> Self {
        Self::exact_as(name, field)
    }

    pub const fn new(name: &'static str, source: &'static str, lookup: Lookup) -> Self {
        Self {
            name,
            source,
            lookup,
            _model: PhantomData,
        }
    }

    pub const fn exact_as(name: &'static str, field: ModelField<M>) -> Self {
        Self::new(name, field.db_name(), Lookup::Exact)
    }
}

#[derive(Debug)]
pub struct FilterSet<M: 'static> {
    filters: &'static [Filter<M>],
    _model: PhantomData<fn() -> M>,
}

impl<M> Default for FilterSet<M> {
    fn default() -> Self {
        Self {
            filters: &[],
            _model: PhantomData,
        }
    }
}

pub trait FilterSetSpec: Clone + Send + Sync + 'static {
    type Model: SqliteModel;

    fn filters(&self) -> &'static [Filter<Self::Model>];

    fn filterset(&self) -> FilterSet<Self::Model> {
        FilterSet::new(self.filters())
    }
}

impl<M> Clone for FilterSet<M> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<M> Copy for FilterSet<M> {}

impl<M> FilterSetSpec for FilterSet<M>
where
    M: SqliteModel,
{
    type Model = M;

    fn filters(&self) -> &'static [Filter<M>] {
        self.filters
    }
}

impl<M> FilterSet<M>
where
    M: SqliteModel,
{
    pub const fn new(filters: &'static [Filter<M>]) -> Self {
        Self {
            filters,
            _model: PhantomData,
        }
    }

    pub fn filters(&self) -> &'static [Filter<M>] {
        self.filters
    }

    pub fn apply<'db>(
        &self,
        mut query: QueryBuilder<'db, M>,
        params: &HashMap<String, String>,
    ) -> AppResult<QueryBuilder<'db, M>> {
        for (name, value) in params {
            match name.as_str() {
                "ordering" => {
                    query = self.apply_ordering(query, value)?;
                }
                "limit" => {
                    query = query.limit(parse_u32("limit", value)?);
                }
                "offset" => {
                    query = query.offset(parse_u32("offset", value)?);
                }
                name => {
                    let filter = self
                        .filters
                        .iter()
                        .find(|filter| filter.matches_query_name(name))
                        .ok_or_else(|| FilterError::UnknownFilter(name.to_string()))?;
                    let field = model_field::<M>(filter.source)?;
                    validate_lookup(field, filter.lookup)?;
                    let value = parse_value(field, value)?;
                    query = match filter.lookup {
                        Lookup::Exact => query.eq(filter.source, value),
                        Lookup::Contains => query.contains(filter.source, value),
                        Lookup::Gt => query.gt(filter.source, value),
                        Lookup::Gte => query.gte(filter.source, value),
                        Lookup::Lt => query.lt(filter.source, value),
                        Lookup::Lte => query.lte(filter.source, value),
                    };
                }
            }
        }

        Ok(query)
    }

    pub fn apply_for_count<'db>(
        &self,
        mut query: QueryBuilder<'db, M>,
        params: &HashMap<String, String>,
    ) -> AppResult<QueryBuilder<'db, M>> {
        for (name, value) in params {
            match name.as_str() {
                "ordering" | "limit" | "offset" => {}
                name => {
                    let filter = self
                        .filters
                        .iter()
                        .find(|filter| filter.matches_query_name(name))
                        .ok_or_else(|| FilterError::UnknownFilter(name.to_string()))?;
                    let field = model_field::<M>(filter.source)?;
                    validate_lookup(field, filter.lookup)?;
                    let value = parse_value(field, value)?;
                    query = match filter.lookup {
                        Lookup::Exact => query.eq(filter.source, value),
                        Lookup::Contains => query.contains(filter.source, value),
                        Lookup::Gt => query.gt(filter.source, value),
                        Lookup::Gte => query.gte(filter.source, value),
                        Lookup::Lt => query.lt(filter.source, value),
                        Lookup::Lte => query.lte(filter.source, value),
                    };
                }
            }
        }

        Ok(query)
    }

    fn apply_ordering<'db>(
        &self,
        query: QueryBuilder<'db, M>,
        value: &str,
    ) -> AppResult<QueryBuilder<'db, M>> {
        let field = value.strip_prefix('-').unwrap_or(value);
        if !self.filters.iter().any(|filter| filter.name == field) {
            return Err(FilterError::UnknownOrdering(field.to_string()).into());
        }
        let source = self
            .filters
            .iter()
            .find(|filter| filter.name == field)
            .map(|filter| filter.source)
            .unwrap_or(field);
        model_field::<M>(source)?;

        Ok(if value.starts_with('-') {
            query.order_by(format!("-{source}").as_str())
        } else {
            query.order_by(source)
        })
    }
}

impl<M> Filter<M> {
    pub fn query_name(&self) -> String {
        match self.lookup {
            Lookup::Exact => self.name.to_string(),
            Lookup::Contains => format!("{}__contains", self.name),
            Lookup::Gt => format!("{}__gt", self.name),
            Lookup::Gte => format!("{}__gte", self.name),
            Lookup::Lt => format!("{}__lt", self.name),
            Lookup::Lte => format!("{}__lte", self.name),
        }
    }

    pub fn matches_query_name(&self, name: &str) -> bool {
        match self.lookup {
            Lookup::Exact => name == self.name,
            Lookup::Contains => name.strip_suffix("__contains") == Some(self.name),
            Lookup::Gt => name.strip_suffix("__gt") == Some(self.name),
            Lookup::Gte => name.strip_suffix("__gte") == Some(self.name),
            Lookup::Lt => name.strip_suffix("__lt") == Some(self.name),
            Lookup::Lte => name.strip_suffix("__lte") == Some(self.name),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FilterError {
    #[error("unknown filter: {0}")]
    UnknownFilter(String),

    #[error("unknown ordering field: {0}")]
    UnknownOrdering(String),

    #[error("invalid filter value for {field}, expected {expected}")]
    InvalidValue {
        field: String,
        expected: &'static str,
    },

    #[error("invalid lookup for field: {0}")]
    InvalidLookup(String),

    #[error("invalid model field: {0}")]
    InvalidModelField(String),
}

fn model_field<M: Model>(name: &str) -> Result<&'static FieldInfo, FilterError> {
    M::fields()
        .iter()
        .find(|field| field.rust_name == name || field.db_name == name)
        .ok_or_else(|| FilterError::InvalidModelField(name.to_string()))
}

fn parse_value(field: &FieldInfo, value: &str) -> Result<SqliteValue, FilterError> {
    match field.ty {
        FieldType::Integer => value
            .parse::<i64>()
            .map(SqliteValue::from)
            .map_err(|_| invalid_value(field, "integer")),
        FieldType::Text | FieldType::FilePath => Ok(SqliteValue::from(value)),
        FieldType::Choice => {
            if field
                .choices
                .is_some_and(|choices| choices.contains(&value))
            {
                Ok(SqliteValue::from(value))
            } else {
                Err(invalid_value(field, "allowed choice"))
            }
        }
        FieldType::Boolean => parse_bool(value)
            .map(SqliteValue::from)
            .ok_or_else(|| invalid_value(field, "boolean")),
        FieldType::Real => value
            .parse::<f64>()
            .map(SqliteValue::from)
            .map_err(|_| invalid_value(field, "number")),
        FieldType::DateTime => parse_datetime(value)
            .map(SqliteValue::from)
            .ok_or_else(|| invalid_value(field, "datetime string")),
        FieldType::Json => serde_json::from_str::<serde_json::Value>(value)
            .map(SqliteValue::from)
            .map_err(|_| invalid_value(field, "json")),
    }
}

fn validate_lookup(field: &FieldInfo, lookup: Lookup) -> Result<(), FilterError> {
    match lookup {
        Lookup::Exact => Ok(()),
        Lookup::Contains if matches!(field.ty, FieldType::Text | FieldType::FilePath) => Ok(()),
        Lookup::Gt | Lookup::Gte | Lookup::Lt | Lookup::Lte
            if matches!(
                field.ty,
                FieldType::Integer | FieldType::Real | FieldType::DateTime
            ) =>
        {
            Ok(())
        }
        _ => Err(FilterError::InvalidLookup(field.rust_name.to_string())),
    }
}

fn parse_datetime(value: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S"))
        .ok()
}

fn parse_bool(value: &str) -> Option<bool> {
    match value {
        "true" | "1" => Some(true),
        "false" | "0" => Some(false),
        _ => None,
    }
}

fn parse_u32(field: &str, value: &str) -> Result<u32, FilterError> {
    value.parse::<u32>().map_err(|_| FilterError::InvalidValue {
        field: field.to_string(),
        expected: "positive integer",
    })
}

fn invalid_value(field: &FieldInfo, expected: &'static str) -> FilterError {
    FilterError::InvalidValue {
        field: field.rust_name.to_string(),
        expected,
    }
}
