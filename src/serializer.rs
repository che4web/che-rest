use std::{future::Future, marker::PhantomData, pin::Pin};

use che_orm::{
    FieldInfo, FieldType, Model, NaiveDateTime, SqliteBackend, SqliteModel, SqliteValue,
};
use serde_json::{Map, Value};

#[derive(Clone, Copy)]
pub struct RelatedSerializer {
    pub(crate) model_name: fn() -> &'static str,
    pub(crate) serialize:
        for<'a> fn(
            &'a SqliteBackend,
            i64,
        ) -> Pin<Box<dyn Future<Output = che_orm::Result<Value>> + Send + 'a>>,
}

impl std::fmt::Debug for RelatedSerializer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RelatedSerializer")
            .field("model", &(self.model_name)())
            .finish_non_exhaustive()
    }
}

impl RelatedSerializer {
    pub const fn of<S>() -> Self
    where
        S: Serializer + Default,
        S::Model: SqliteModel<Id = i64>,
    {
        Self {
            model_name: related_model_name::<S::Model>,
            serialize: related_serialize::<S>,
        }
    }

    pub fn model_name(&self) -> &'static str {
        (self.model_name)()
    }

    pub fn serialize<'a>(
        &self,
        db: &'a SqliteBackend,
        id: i64,
    ) -> Pin<Box<dyn Future<Output = che_orm::Result<Value>> + Send + 'a>> {
        (self.serialize)(db, id)
    }
}

fn related_model_name<M>() -> &'static str {
    std::any::type_name::<M>()
        .rsplit("::")
        .next()
        .unwrap_or("Model")
}

fn related_serialize<'a, S>(
    db: &'a SqliteBackend,
    id: i64,
) -> Pin<Box<dyn Future<Output = che_orm::Result<Value>> + Send + 'a>>
where
    S: Serializer + Default,
    S::Model: SqliteModel<Id = i64>,
{
    Box::pin(async move {
        let model = S::Model::objects(db).get(id).await?;
        S::default()
            .model_serializer()
            .to_json_async(db, &model)
            .await
    })
}

#[derive(Debug, Clone, Copy)]
pub struct Field {
    pub name: &'static str,
    pub source: &'static str,
    pub required: bool,
    pub read_only: bool,
    pub write_only: bool,
    pub nullable: bool,
    pub max_length: Option<u32>,
    pub relation: Option<RelatedSerializer>,
    pub system: bool,
    default: Option<fn() -> Value>,
}

impl Field {
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            source: name,
            required: true,
            read_only: false,
            write_only: false,
            nullable: false,
            max_length: None,
            relation: None,
            system: false,
            default: None,
        }
    }

    pub const fn related<S>(name: &'static str, source: &'static str) -> Self
    where
        S: Serializer + Default,
        S::Model: SqliteModel<Id = i64>,
    {
        Self {
            name,
            source,
            required: false,
            read_only: true,
            write_only: false,
            nullable: false,
            max_length: None,
            relation: Some(RelatedSerializer::of::<S>()),
            system: false,
            default: None,
        }
    }

    pub const fn source(mut self, source: &'static str) -> Self {
        self.source = source;
        self
    }

    pub const fn required(mut self, required: bool) -> Self {
        self.required = required;
        self
    }

    pub const fn read_only(mut self) -> Self {
        self.read_only = true;
        self.required = false;
        self
    }

    pub const fn write_only(mut self) -> Self {
        self.write_only = true;
        self
    }

    pub const fn system(mut self) -> Self {
        self.system = true;
        self.read_only = true;
        self.write_only = true;
        self.required = false;
        self
    }

    pub const fn nullable(mut self) -> Self {
        self.nullable = true;
        self
    }

    pub const fn max_length(mut self, max_length: u32) -> Self {
        self.max_length = Some(max_length);
        self
    }

    pub const fn default(mut self, default: fn() -> Value) -> Self {
        self.default = Some(default);
        self
    }

    pub const fn without_default(mut self) -> Self {
        self.default = None;
        self
    }

    pub const fn has_default(&self) -> bool {
        self.default.is_some()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SerializerError {
    #[error("expected a JSON object")]
    ExpectedObject,

    #[error("unknown field: {0}")]
    UnknownField(String),

    #[error("field is read-only: {0}")]
    ReadonlyField(String),

    #[error("missing field: {0}")]
    MissingField(String),

    #[error("null is not allowed for field: {0}")]
    NullNotAllowed(String),

    #[error("invalid type for field {field}, expected {expected}")]
    InvalidType {
        field: String,
        expected: &'static str,
    },

    #[error("field {field} exceeds max length {max_length}")]
    MaxLengthExceeded { field: String, max_length: u32 },

    #[error("invalid model field: {0}")]
    InvalidModelField(String),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, SerializerError>;

#[derive(Debug)]
pub struct ModelSerializer<M> {
    fields: &'static [Field],
    _model: PhantomData<M>,
}

impl<M> Clone for ModelSerializer<M> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<M> Copy for ModelSerializer<M> {}

impl<M: Model> Default for ModelSerializer<M> {
    fn default() -> Self {
        Self::new(&[])
    }
}

pub trait Serializer: Clone + Send + Sync + 'static {
    type Model: Model;

    fn fields(&self) -> &'static [Field];

    fn model_serializer(&self) -> ModelSerializer<Self::Model> {
        ModelSerializer::new(self.fields())
    }
}

impl<M> Serializer for ModelSerializer<M>
where
    M: Model,
{
    type Model = M;

    fn fields(&self) -> &'static [Field] {
        self.fields
    }
}

impl<M: Model> ModelSerializer<M> {
    pub const fn new(fields: &'static [Field]) -> Self {
        Self {
            fields,
            _model: PhantomData,
        }
    }

    pub fn fields(&self) -> &'static [Field] {
        self.fields
    }

    pub fn to_json(&self, model: &M) -> Value {
        serialize_model(model, self.fields)
    }

    pub async fn to_json_async(&self, db: &SqliteBackend, model: &M) -> che_orm::Result<Value> {
        serialize_model_async(db, model, self.fields).await
    }

    pub fn validate_json(&self, value: Value) -> Result<Map<String, Value>> {
        validate_object::<M>(value, self.fields)
    }

    pub fn create_values(&self, value: Value) -> Result<Vec<(&'static str, SqliteValue)>> {
        validated_values::<M>(value, self.fields)
    }

    pub fn update_values(&self, value: Value) -> Result<Vec<(&'static str, SqliteValue)>> {
        let fields = self
            .fields
            .iter()
            .map(|field| {
                if field.read_only {
                    *field
                } else {
                    field.required(false).without_default()
                }
            })
            .collect::<Vec<_>>();

        validated_values::<M>(value, &fields)
    }

    pub fn system_create_values(&self, value: Value) -> Result<Vec<(&'static str, SqliteValue)>> {
        validated_system_values::<M>(value, self.fields)
    }
}

pub fn serialize_model<M: Model>(model: &M, fields: &[Field]) -> Value {
    let mut object = Map::new();

    for field in fields {
        if field.write_only {
            continue;
        }

        if field.relation.is_some() {
            continue;
        }

        let value = model
            .get_value(field.source)
            .or_else(|| model.get_value(field.name))
            .unwrap_or(Value::Null);
        object.insert(field.name.to_string(), value);
    }

    Value::Object(object)
}

pub async fn serialize_model_async<M: Model>(
    db: &SqliteBackend,
    model: &M,
    fields: &[Field],
) -> che_orm::Result<Value> {
    let mut object = Map::new();

    for field in fields {
        if field.write_only {
            continue;
        }

        if let Some(relation) = field.relation {
            let value = match model
                .get_value(field.source)
                .or_else(|| model.get_value(field.name))
                .and_then(|value| value.as_i64())
            {
                Some(id) => relation.serialize(db, id).await?,
                None => Value::Null,
            };
            object.insert(field.name.to_string(), value);
            continue;
        }

        let value = model
            .get_value(field.source)
            .or_else(|| model.get_value(field.name))
            .unwrap_or(Value::Null);
        object.insert(field.name.to_string(), value);
    }

    Ok(Value::Object(object))
}

pub fn validate_object<M: Model>(value: Value, fields: &[Field]) -> Result<Map<String, Value>> {
    validate_object_internal::<M>(value, fields)
}

fn validate_object_internal<M: Model>(
    value: Value,
    fields: &[Field],
) -> Result<Map<String, Value>> {
    let object = value.as_object().ok_or(SerializerError::ExpectedObject)?;

    for key in object.keys() {
        if !fields.iter().any(|field| field.name == key.as_str()) {
            return Err(SerializerError::UnknownField(key.clone()));
        }
    }

    let model_fields = M::fields();
    let mut validated = Map::new();

    for field in fields {
        let model_field = find_model_field(model_fields, field.source)
            .ok_or_else(|| SerializerError::InvalidModelField(field.source.to_string()))?;

        if field.read_only {
            if object.contains_key(field.name) {
                return Err(SerializerError::ReadonlyField(field.name.to_string()));
            }
            continue;
        }

        match object.get(field.name) {
            Some(Value::Null) => {
                if !field.nullable {
                    return Err(SerializerError::NullNotAllowed(field.name.to_string()));
                }
                validated.insert(field.name.to_string(), Value::Null);
            }
            Some(value) => {
                validate_type(field.name, model_field.ty, value)?;
                if let Some(max_length) = field.max_length {
                    validate_max_length(field.name, max_length, value)?;
                }
                validated.insert(field.name.to_string(), value.clone());
            }
            None => {
                if let Some(default) = field.default {
                    validated.insert(field.name.to_string(), default());
                } else if field.required {
                    return Err(SerializerError::MissingField(field.name.to_string()));
                }
            }
        }
    }

    Ok(validated)
}

fn validated_values<M: Model>(
    value: Value,
    fields: &[Field],
) -> Result<Vec<(&'static str, SqliteValue)>> {
    let data = validate_object_internal::<M>(value, fields)?;
    let mut values = Vec::new();

    for (name, value) in data {
        let field = find_model_field(M::fields(), &name)
            .ok_or_else(|| SerializerError::InvalidModelField(name.clone()))?;
        values.push((field.db_name, json_to_sqlite_value(field, value)?));
    }

    Ok(values)
}

fn validated_system_values<M: Model>(
    value: Value,
    fields: &[Field],
) -> Result<Vec<(&'static str, SqliteValue)>> {
    let object = value.as_object().ok_or(SerializerError::ExpectedObject)?;
    let mut values = Vec::new();

    for (name, value) in object {
        let field = fields
            .iter()
            .find(|field| field.name == name)
            .ok_or_else(|| SerializerError::UnknownField(name.clone()))?;
        if !field.system {
            return Err(SerializerError::ReadonlyField(name.clone()));
        }

        let model_field = find_model_field(M::fields(), field.source)
            .ok_or_else(|| SerializerError::InvalidModelField(field.source.to_string()))?;
        if value.is_null() {
            if !field.nullable {
                return Err(SerializerError::NullNotAllowed(name.clone()));
            }
        } else {
            validate_type(name, model_field.ty, value)?;
            if let Some(max_length) = field.max_length {
                validate_max_length(name, max_length, value)?;
            }
        }

        values.push((
            model_field.db_name,
            json_to_sqlite_value(model_field, value.clone())?,
        ));
    }

    Ok(values)
}

fn json_to_sqlite_value(field: &FieldInfo, value: Value) -> Result<SqliteValue> {
    if value.is_null() {
        return Ok(SqliteValue::Null);
    }

    match field.ty {
        FieldType::Integer => value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
            .map(SqliteValue::from)
            .ok_or_else(|| SerializerError::InvalidType {
                field: field.rust_name.to_string(),
                expected: "integer",
            }),
        FieldType::Text => {
            value
                .as_str()
                .map(SqliteValue::from)
                .ok_or_else(|| SerializerError::InvalidType {
                    field: field.rust_name.to_string(),
                    expected: "string",
                })
        }
        FieldType::Boolean => {
            value
                .as_bool()
                .map(SqliteValue::from)
                .ok_or_else(|| SerializerError::InvalidType {
                    field: field.rust_name.to_string(),
                    expected: "boolean",
                })
        }
        FieldType::Real => {
            value
                .as_f64()
                .map(SqliteValue::from)
                .ok_or_else(|| SerializerError::InvalidType {
                    field: field.rust_name.to_string(),
                    expected: "number",
                })
        }
        FieldType::DateTime => value
            .as_str()
            .and_then(parse_datetime)
            .map(SqliteValue::from)
            .ok_or_else(|| SerializerError::InvalidType {
                field: field.rust_name.to_string(),
                expected: "datetime string",
            }),
        FieldType::Json => Ok(SqliteValue::from(value)),
    }
}

fn find_model_field<'a>(fields: &'a [FieldInfo], name: &str) -> Option<&'a FieldInfo> {
    fields
        .iter()
        .find(|field| field.rust_name == name || field.db_name == name)
}

fn validate_type(field: &str, ty: FieldType, value: &Value) -> Result<()> {
    let expected = match ty {
        FieldType::Integer => {
            if value.as_i64().is_some() || value.as_u64().is_some() {
                return Ok(());
            }
            "integer"
        }
        FieldType::Text => {
            if value.as_str().is_some() {
                return Ok(());
            }
            "string"
        }
        FieldType::Boolean => {
            if value.as_bool().is_some() {
                return Ok(());
            }
            "boolean"
        }
        FieldType::Real => {
            if value.as_f64().is_some() {
                return Ok(());
            }
            "number"
        }
        FieldType::DateTime => {
            if value.as_str().and_then(parse_datetime).is_some() {
                return Ok(());
            }
            "datetime string"
        }
        FieldType::Json => return Ok(()),
    };

    Err(SerializerError::InvalidType {
        field: field.to_string(),
        expected,
    })
}

fn parse_datetime(value: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S"))
        .ok()
}

fn validate_max_length(field: &str, max_length: u32, value: &Value) -> Result<()> {
    let Some(text) = value.as_str() else {
        return Ok(());
    };

    if text.chars().count() > max_length as usize {
        return Err(SerializerError::MaxLengthExceeded {
            field: field.to_string(),
            max_length,
        });
    }

    Ok(())
}
