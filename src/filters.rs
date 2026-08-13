use std::{collections::HashMap, marker::PhantomData};

use che_orm::{
    Choice, ContainsQueryValue, FilePath, ModelField, NaiveDateTime, QueryBuilder, QueryValue,
    SqliteModel,
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

type ApplyFilter<M> = for<'db> fn(
    QueryBuilder<'db, M>,
    &'static str,
    &str,
) -> Result<QueryBuilder<'db, M>, FilterError>;
type ApplyOrdering<M> =
    for<'db> fn(QueryBuilder<'db, M>, &'static str, bool) -> QueryBuilder<'db, M>;

#[derive(Debug, Clone, Copy)]
pub struct Filter<M: SqliteModel> {
    pub name: &'static str,
    pub source: &'static str,
    pub lookup: Lookup,
    field: &'static str,
    apply: ApplyFilter<M>,
    order: ApplyOrdering<M>,
    _model: PhantomData<fn() -> M>,
}

pub trait FilterValue: Sized + QueryValue<Self> {
    const EXPECTED: &'static str;

    fn parse_filter(value: &str) -> Result<Self, FilterError>;
}

pub trait RangeFilterValue: FilterValue {}

macro_rules! parse_filter_value {
    ($($ty:ty => $expected:literal),+ $(,)?) => {
        $(
            impl FilterValue for $ty {
                const EXPECTED: &'static str = $expected;

                fn parse_filter(value: &str) -> Result<Self, FilterError> {
                    value.parse().map_err(|_| FilterError::InvalidValue {
                        field: String::new(),
                        expected: Self::EXPECTED,
                    })
                }
            }
        )+
    };
}

parse_filter_value!(i64 => "integer", i32 => "integer", u32 => "integer");
parse_filter_value!(f64 => "number", f32 => "number");

impl RangeFilterValue for i64 {}
impl RangeFilterValue for i32 {}
impl RangeFilterValue for u32 {}
impl RangeFilterValue for f64 {}
impl RangeFilterValue for f32 {}

impl FilterValue for String {
    const EXPECTED: &'static str = "string";

    fn parse_filter(value: &str) -> Result<Self, FilterError> {
        Ok(value.to_string())
    }
}

impl FilterValue for bool {
    const EXPECTED: &'static str = "boolean";

    fn parse_filter(value: &str) -> Result<Self, FilterError> {
        match value {
            "true" | "1" => Ok(true),
            "false" | "0" => Ok(false),
            _ => Err(FilterError::InvalidValue {
                field: String::new(),
                expected: Self::EXPECTED,
            }),
        }
    }
}

impl FilterValue for NaiveDateTime {
    const EXPECTED: &'static str = "datetime string";

    fn parse_filter(value: &str) -> Result<Self, FilterError> {
        NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S")
            .or_else(|_| NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S"))
            .map_err(|_| FilterError::InvalidValue {
                field: String::new(),
                expected: Self::EXPECTED,
            })
    }
}

impl RangeFilterValue for NaiveDateTime {}

impl FilterValue for serde_json::Value {
    const EXPECTED: &'static str = "json";

    fn parse_filter(value: &str) -> Result<Self, FilterError> {
        serde_json::from_str(value).map_err(|_| FilterError::InvalidValue {
            field: String::new(),
            expected: Self::EXPECTED,
        })
    }
}

impl FilterValue for FilePath {
    const EXPECTED: &'static str = "file path";

    fn parse_filter(value: &str) -> Result<Self, FilterError> {
        FilePath::new(value).map_err(|_| FilterError::InvalidValue {
            field: String::new(),
            expected: Self::EXPECTED,
        })
    }
}

impl<M> Filter<M>
where
    M: SqliteModel,
{
    pub const fn exact<T>(field: ModelField<M, T>) -> Self
    where
        T: FilterValue,
    {
        Self::typed(field, Lookup::Exact, apply_exact::<M, T>)
    }

    pub const fn contains<T>(field: ModelField<M, T>) -> Self
    where
        T: FilterValue + ContainsQueryValue,
    {
        Self::typed(field, Lookup::Contains, apply_contains::<M, T>)
    }

    pub const fn gt<T>(field: ModelField<M, T>) -> Self
    where
        T: RangeFilterValue,
    {
        Self::typed(field, Lookup::Gt, apply_gt::<M, T>)
    }

    pub const fn gte<T>(field: ModelField<M, T>) -> Self
    where
        T: RangeFilterValue,
    {
        Self::typed(field, Lookup::Gte, apply_gte::<M, T>)
    }

    pub const fn lt<T>(field: ModelField<M, T>) -> Self
    where
        T: RangeFilterValue,
    {
        Self::typed(field, Lookup::Lt, apply_lt::<M, T>)
    }

    pub const fn lte<T>(field: ModelField<M, T>) -> Self
    where
        T: RangeFilterValue,
    {
        Self::typed(field, Lookup::Lte, apply_lte::<M, T>)
    }

    pub const fn exact_source<T>(name: &'static str, field: ModelField<M, T>) -> Self
    where
        T: FilterValue,
    {
        Self::exact_as(name, field)
    }

    pub const fn exact_as<T>(name: &'static str, field: ModelField<M, T>) -> Self
    where
        T: FilterValue,
    {
        let mut filter = Self::exact(field);
        filter.name = name;
        filter
    }

    pub const fn exact_choice<T>(field: ModelField<M, T>) -> Self
    where
        T: Choice + QueryValue<T>,
    {
        Self::typed(field, Lookup::Exact, apply_choice_exact::<M, T>)
    }

    const fn typed<T>(field: ModelField<M, T>, lookup: Lookup, apply: ApplyFilter<M>) -> Self {
        Self {
            name: field.db_name(),
            source: field.db_name(),
            lookup,
            field: field.db_name(),
            apply,
            order: apply_ordering::<M, T>,
            _model: PhantomData,
        }
    }
}

#[derive(Debug)]
pub struct FilterSet<M: SqliteModel> {
    filters: &'static [Filter<M>],
    _model: PhantomData<fn() -> M>,
}

impl<M: SqliteModel> Default for FilterSet<M> {
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

impl<M: SqliteModel> Clone for FilterSet<M> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<M: SqliteModel> Copy for FilterSet<M> {}

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
                "ordering" => query = self.apply_ordering(query, value)?,
                "limit" => query = query.limit(parse_u32("limit", value)?),
                "offset" => query = query.offset(parse_u32("offset", value)?),
                name => query = self.apply_filter(query, name, value)?,
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
            if !matches!(name.as_str(), "ordering" | "limit" | "offset") {
                query = self.apply_filter(query, name, value)?;
            }
        }

        Ok(query)
    }

    fn apply_filter<'db>(
        &self,
        query: QueryBuilder<'db, M>,
        name: &str,
        value: &str,
    ) -> AppResult<QueryBuilder<'db, M>> {
        let filter = self
            .filters
            .iter()
            .find(|filter| filter.matches_query_name(name))
            .ok_or_else(|| FilterError::UnknownFilter(name.to_string()))?;
        let query = (filter.apply)(query, filter.field, value).map_err(|error| match error {
            FilterError::InvalidValue { expected, .. } => FilterError::InvalidValue {
                field: filter.name.to_string(),
                expected,
            },
            error => error,
        })?;
        Ok(query)
    }

    fn apply_ordering<'db>(
        &self,
        query: QueryBuilder<'db, M>,
        value: &str,
    ) -> AppResult<QueryBuilder<'db, M>> {
        let name = value.strip_prefix('-').unwrap_or(value);
        let filter = self
            .filters
            .iter()
            .find(|filter| filter.name == name)
            .ok_or_else(|| FilterError::UnknownOrdering(name.to_string()))?;
        Ok((filter.order)(query, filter.field, value.starts_with('-')))
    }
}

impl<M: SqliteModel> Filter<M> {
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
}

fn apply_exact<'db, M, T>(
    query: QueryBuilder<'db, M>,
    name: &'static str,
    value: &str,
) -> Result<QueryBuilder<'db, M>, FilterError>
where
    M: SqliteModel,
    T: FilterValue,
{
    let field = unsafe { ModelField::<M, T>::new(name) };
    Ok(query.filter(field.eq(T::parse_filter(value)?)))
}

fn apply_contains<'db, M, T>(
    query: QueryBuilder<'db, M>,
    name: &'static str,
    value: &str,
) -> Result<QueryBuilder<'db, M>, FilterError>
where
    M: SqliteModel,
    T: FilterValue + ContainsQueryValue,
{
    let field = unsafe { ModelField::<M, T>::new(name) };
    Ok(query.filter(field.contains(T::parse_filter(value)?)))
}

fn apply_choice_exact<'db, M, T>(
    query: QueryBuilder<'db, M>,
    name: &'static str,
    value: &str,
) -> Result<QueryBuilder<'db, M>, FilterError>
where
    M: SqliteModel,
    T: Choice + QueryValue<T>,
{
    let field = unsafe { ModelField::<M, T>::new(name) };
    let value = T::from_str(value).map_err(|_| FilterError::InvalidValue {
        field: String::new(),
        expected: "allowed choice",
    })?;
    Ok(query.filter(field.eq(value)))
}

macro_rules! apply_range {
    ($name:ident, $method:ident) => {
        fn $name<'db, M, T>(
            query: QueryBuilder<'db, M>,
            name: &'static str,
            value: &str,
        ) -> Result<QueryBuilder<'db, M>, FilterError>
        where
            M: SqliteModel,
            T: RangeFilterValue,
        {
            let field = unsafe { ModelField::<M, T>::new(name) };
            Ok(query.filter(field.$method(T::parse_filter(value)?)))
        }
    };
}

apply_range!(apply_gt, gt);
apply_range!(apply_gte, gte);
apply_range!(apply_lt, lt);
apply_range!(apply_lte, lte);

fn apply_ordering<'db, M, T>(
    query: QueryBuilder<'db, M>,
    name: &'static str,
    descending: bool,
) -> QueryBuilder<'db, M>
where
    M: SqliteModel,
{
    let field = unsafe { ModelField::<M, T>::new(name) };
    if descending {
        query.order_by_desc(field)
    } else {
        query.order_by(field)
    }
}

fn parse_u32(field: &str, value: &str) -> Result<u32, FilterError> {
    value.parse::<u32>().map_err(|_| FilterError::InvalidValue {
        field: field.to_string(),
        expected: "positive integer",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use che_orm::Database;

    use crate::auth::models::{User, UserFields};

    static USER_FILTERS: &[Filter<User>] = &[
        Filter::exact(UserFields::USERNAME),
        Filter::contains(UserFields::USERNAME),
    ];

    #[tokio::test]
    async fn applies_typed_filter_and_ordering() {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        db.create_table::<User>().await.unwrap();
        db.create::<User>()
            .set(UserFields::USERNAME, "alice")
            .set(UserFields::PASSWORD_HASH, "hash")
            .execute()
            .await
            .unwrap();
        db.create::<User>()
            .set(UserFields::USERNAME, "alex")
            .set(UserFields::PASSWORD_HASH, "hash")
            .execute()
            .await
            .unwrap();

        let params = HashMap::from([
            ("username__contains".to_string(), "al".to_string()),
            ("ordering".to_string(), "-username".to_string()),
        ]);
        let users = FilterSet::new(USER_FILTERS)
            .apply(db.query::<User>(), &params)
            .unwrap()
            .all()
            .await
            .unwrap();

        assert_eq!(users.len(), 2);
        assert_eq!(users[0].username, "alice");
        assert_eq!(users[1].username, "alex");
    }
}
