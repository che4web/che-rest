use std::{future::Future, marker::PhantomData, pin::Pin};

use async_trait::async_trait;
use che_orm::{
    Choice, Database, DatabaseCreateBuilder, DatabaseUpdateBuilder, FieldInfo, FieldType, FilePath,
    Model, ModelField, NaiveDateTime, SqliteModel,
};
use serde_json::{Map, Value};

#[derive(Clone, Copy)]
pub struct RelatedSerializer {
    pub(crate) model_name: fn() -> &'static str,
    pub(crate) serialize:
        for<'a> fn(
            &'a Database,
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
        db: &'a Database,
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
    db: &'a Database,
    id: i64,
) -> Pin<Box<dyn Future<Output = che_orm::Result<Value>> + Send + 'a>>
where
    S: Serializer + Default,
    S::Model: SqliteModel<Id = i64>,
{
    Box::pin(async move {
        let model = db.get::<S::Model>(id).await?;
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

    #[error("invalid choice for field {field}: {value}")]
    InvalidChoice { field: String, value: String },

    #[error("field {field} exceeds max length {max_length}")]
    MaxLengthExceeded { field: String, max_length: u32 },

    #[error("invalid model field: {0}")]
    InvalidModelField(String),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, SerializerError>;

pub struct ValidatedData<M> {
    values: Map<String, Value>,
    _model: PhantomData<fn() -> M>,
}

impl<M> ValidatedData<M> {
    fn new(values: Map<String, Value>) -> Self {
        Self {
            values,
            _model: PhantomData,
        }
    }

    pub fn merge(mut self, values: Self) -> Self {
        self.values.extend(values.values);
        self
    }

    pub fn required<T>(&self, field: ModelField<M, T>) -> Result<T>
    where
        T: ValidatedValue,
    {
        let value = self
            .values
            .get(field.db_name())
            .ok_or_else(|| SerializerError::MissingField(field.db_name().to_string()))?;
        T::from_validated(field.db_name(), value)
    }

    pub fn optional<T>(&self, field: ModelField<M, T>) -> Result<Option<T>>
    where
        T: ValidatedValue,
    {
        self.values
            .get(field.db_name())
            .map(|value| T::from_validated(field.db_name(), value))
            .transpose()
    }

    fn apply_create<'db>(
        self,
        mut create: DatabaseCreateBuilder<'db, M>,
    ) -> crate::AppResult<DatabaseCreateBuilder<'db, M>>
    where
        M: SqliteModel,
    {
        for (field, value) in self.values {
            create = create.set_value(&field, json_to_database_value::<M>(&field, value)?)?;
        }
        Ok(create)
    }

    fn apply_update<'db>(
        self,
        mut update: DatabaseUpdateBuilder<'db, M>,
    ) -> crate::AppResult<DatabaseUpdateBuilder<'db, M>>
    where
        M: SqliteModel,
    {
        for (field, value) in self.values {
            update = update.set_value(&field, json_to_database_value::<M>(&field, value)?)?;
        }
        Ok(update)
    }
}

fn json_to_database_value<M: Model>(field: &str, value: Value) -> Result<che_orm::DatabaseValue> {
    let info = find_model_field(M::fields(), field)
        .ok_or_else(|| SerializerError::InvalidModelField(field.to_string()))?;
    if value.is_null() {
        return Ok(che_orm::DatabaseValue::Null);
    }
    match info.ty {
        FieldType::Integer => value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
            .map(Into::into),
        FieldType::Text | FieldType::Choice => value.as_str().map(Into::into),
        FieldType::FilePath => value
            .as_str()
            .and_then(|value| FilePath::new(value).ok())
            .map(Into::into),
        FieldType::Boolean => value.as_bool().map(Into::into),
        FieldType::Real => value.as_f64().map(Into::into),
        FieldType::DateTime => value.as_str().and_then(parse_datetime).map(Into::into),
        FieldType::Json => Some(value.into()),
    }
    .ok_or_else(|| SerializerError::InvalidType {
        field: field.to_string(),
        expected: field_type_name(info.ty),
    })
}

fn field_type_name(ty: FieldType) -> &'static str {
    match ty {
        FieldType::Integer => "integer",
        FieldType::Text | FieldType::Choice | FieldType::FilePath => "string",
        FieldType::Boolean => "boolean",
        FieldType::Real => "number",
        FieldType::DateTime => "datetime string",
        FieldType::Json => "json",
    }
}

pub trait ValidatedValue: Sized {
    fn from_validated(field: &str, value: &Value) -> Result<Self>;
}

macro_rules! validated_number {
    ($($ty:ty => $expected:literal),+ $(,)?) => {$(
        impl ValidatedValue for $ty {
            fn from_validated(field: &str, value: &Value) -> Result<Self> {
                value.as_i64().and_then(|value| value.try_into().ok()).ok_or_else(|| {
                    SerializerError::InvalidType { field: field.to_string(), expected: $expected }
                })
            }
        }
    )+};
}

validated_number!(i64 => "integer", i32 => "integer", u32 => "integer");

impl ValidatedValue for String {
    fn from_validated(field: &str, value: &Value) -> Result<Self> {
        value
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| SerializerError::InvalidType {
                field: field.to_string(),
                expected: "string",
            })
    }
}

impl ValidatedValue for bool {
    fn from_validated(field: &str, value: &Value) -> Result<Self> {
        value.as_bool().ok_or_else(|| SerializerError::InvalidType {
            field: field.to_string(),
            expected: "boolean",
        })
    }
}

impl ValidatedValue for f64 {
    fn from_validated(field: &str, value: &Value) -> Result<Self> {
        value.as_f64().ok_or_else(|| SerializerError::InvalidType {
            field: field.to_string(),
            expected: "number",
        })
    }
}

impl ValidatedValue for f32 {
    fn from_validated(field: &str, value: &Value) -> Result<Self> {
        value
            .as_f64()
            .map(|value| value as f32)
            .ok_or_else(|| SerializerError::InvalidType {
                field: field.to_string(),
                expected: "number",
            })
    }
}

impl ValidatedValue for NaiveDateTime {
    fn from_validated(field: &str, value: &Value) -> Result<Self> {
        value
            .as_str()
            .and_then(parse_datetime)
            .ok_or_else(|| SerializerError::InvalidType {
                field: field.to_string(),
                expected: "datetime string",
            })
    }
}

impl ValidatedValue for Value {
    fn from_validated(_field: &str, value: &Value) -> Result<Self> {
        Ok(value.clone())
    }
}

impl ValidatedValue for FilePath {
    fn from_validated(field: &str, value: &Value) -> Result<Self> {
        let value = String::from_validated(field, value)?;
        FilePath::new(value).map_err(|_| SerializerError::InvalidType {
            field: field.to_string(),
            expected: "file path",
        })
    }
}

impl<M> ValidatedData<M> {
    pub fn choice<T: Choice>(&self, field: ModelField<M, T>) -> Result<T> {
        let value = self
            .values
            .get(field.db_name())
            .ok_or_else(|| SerializerError::MissingField(field.db_name().to_string()))?
            .as_str()
            .ok_or_else(|| SerializerError::InvalidType {
                field: field.db_name().to_string(),
                expected: "string",
            })?;
        T::from_str(value).map_err(|_| SerializerError::InvalidChoice {
            field: field.db_name().to_string(),
            value: value.to_string(),
        })
    }

    pub fn optional_choice<T: Choice>(&self, field: ModelField<M, T>) -> Result<Option<T>> {
        let Some(value) = self.values.get(field.db_name()) else {
            return Ok(None);
        };
        let value = value.as_str().ok_or_else(|| SerializerError::InvalidType {
            field: field.db_name().to_string(),
            expected: "string",
        })?;
        T::from_str(value)
            .map(Some)
            .map_err(|_| SerializerError::InvalidChoice {
                field: field.db_name().to_string(),
                value: value.to_string(),
            })
    }
}

impl<T: ValidatedValue> ValidatedValue for Option<T> {
    fn from_validated(field: &str, value: &Value) -> Result<Self> {
        if value.is_null() {
            Ok(None)
        } else {
            T::from_validated(field, value).map(Some)
        }
    }
}

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

#[async_trait]
pub trait Serializer: Clone + Send + Sync + 'static {
    type Model: SqliteModel<Id = i64>;

    fn fields(&self) -> &'static [Field];

    fn model_serializer(&self) -> ModelSerializer<Self::Model> {
        ModelSerializer::new(self.fields())
    }

    async fn create(
        &self,
        db: &Database,
        validated: ValidatedData<Self::Model>,
    ) -> crate::AppResult<Self::Model> {
        validated
            .apply_create(db.create::<Self::Model>())?
            .execute()
            .await
            .map_err(Into::into)
    }

    async fn update(
        &self,
        db: &Database,
        instance: Self::Model,
        validated: ValidatedData<Self::Model>,
    ) -> crate::AppResult<Self::Model> {
        validated
            .apply_update(db.update::<Self::Model>(instance.id()))?
            .execute()
            .await
            .map_err(Into::into)
    }
}

#[async_trait]
impl<M> Serializer for ModelSerializer<M>
where
    M: SqliteModel<Id = i64>,
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

    pub async fn to_json_async(&self, db: &Database, model: &M) -> che_orm::Result<Value> {
        serialize_model_async(db, model, self.fields).await
    }

    pub fn validate_json(&self, value: Value) -> Result<Map<String, Value>> {
        validate_object::<M>(value, self.fields)
    }

    pub fn create_data(&self, value: Value) -> Result<ValidatedData<M>> {
        validated_data::<M>(value, self.fields)
    }

    pub fn update_data(&self, value: Value) -> Result<ValidatedData<M>> {
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

        validated_data::<M>(value, &fields)
    }

    pub fn system_create_data(&self, value: Value) -> Result<ValidatedData<M>> {
        validated_system_data::<M>(value, self.fields)
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
    db: &Database,
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
                validate_choice(field.name, model_field, value)?;
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

fn validated_data<M: Model>(value: Value, fields: &[Field]) -> Result<ValidatedData<M>> {
    let data = validate_object_internal::<M>(value, fields)?;
    let mut values = Map::new();

    for field in fields {
        if let Some(value) = data.get(field.name) {
            let model_field = find_model_field(M::fields(), field.source)
                .ok_or_else(|| SerializerError::InvalidModelField(field.source.to_string()))?;
            values.insert(model_field.db_name.to_string(), value.clone());
        }
    }

    Ok(ValidatedData::new(values))
}

fn validated_system_data<M: Model>(value: Value, fields: &[Field]) -> Result<ValidatedData<M>> {
    let object = value.as_object().ok_or(SerializerError::ExpectedObject)?;
    let mut values = Map::new();

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
            validate_choice(name, model_field, value)?;
            if let Some(max_length) = field.max_length {
                validate_max_length(name, max_length, value)?;
            }
        }

        values.insert(model_field.db_name.to_string(), value.clone());
    }

    Ok(ValidatedData::new(values))
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
        FieldType::Text | FieldType::Choice | FieldType::FilePath => {
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

fn validate_choice(field: &str, model_field: &FieldInfo, value: &Value) -> Result<()> {
    if model_field.ty != FieldType::Choice {
        return Ok(());
    }

    let value = value.as_str().ok_or_else(|| SerializerError::InvalidType {
        field: field.to_string(),
        expected: "string",
    })?;
    if model_field
        .choices
        .is_some_and(|choices| choices.contains(&value))
    {
        Ok(())
    } else {
        Err(SerializerError::InvalidChoice {
            field: field.to_string(),
            value: value.to_string(),
        })
    }
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

#[cfg(test)]
mod serializer_tests {
    use super::*;
    use crate::auth::models::{User, UserFields};

    static ALIAS_FIELDS: &[Field] = &[
        Field::new("id").read_only(),
        Field::new("handle").source("username"),
    ];

    #[test]
    fn validated_data_uses_serializer_source_for_aliases() {
        let serializer = ModelSerializer::<User>::new(ALIAS_FIELDS);
        let data = serializer
            .create_data(serde_json::json!({ "handle": "alice" }))
            .unwrap();

        assert_eq!(data.required(UserFields::USERNAME).unwrap(), "alice");
    }
}
