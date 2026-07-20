use serde_json::{Map, Value};

use crate::User;
use crate::serializer::{Field, Result, Serializer, SerializerError, validate_object};

pub struct UserPublicSerializer;

pub struct UserCreateSerializer;

pub struct UserUpdateSerializer;

impl Serializer<User> for UserPublicSerializer {
    fn fields() -> &'static [Field] {
        static FIELDS: &[Field] = &[
            Field::new("id").read_only(),
            Field::new("email").max_length(255),
            Field::new("name"),
            Field::new("is_active"),
        ];

        FIELDS
    }
}

impl Serializer<User> for UserCreateSerializer {
    fn fields() -> &'static [Field] {
        static FIELDS: &[Field] = &[
            Field::new("email").max_length(255),
            Field::new("name"),
            Field::new("is_active")
                .required(false)
                .default(default_true),
        ];

        FIELDS
    }

    fn validate_json(value: Value) -> Result<Map<String, Value>> {
        validate_object::<User>(value, Self::fields())
    }
}

impl Serializer<User> for UserUpdateSerializer {
    fn fields() -> &'static [Field] {
        static FIELDS: &[Field] = &[
            Field::new("email").required(false).max_length(255),
            Field::new("name").required(false),
            Field::new("is_active").required(false),
        ];

        FIELDS
    }
}

impl UserPublicSerializer {
    pub fn fields() -> &'static [Field] {
        <Self as Serializer<User>>::fields()
    }

    pub fn to_json(model: &User) -> Value {
        <Self as Serializer<User>>::to_json(model)
    }
}

impl UserCreateSerializer {
    pub fn fields() -> &'static [Field] {
        <Self as Serializer<User>>::fields()
    }

    pub fn validate_json(value: Value) -> Result<Map<String, Value>> {
        <Self as Serializer<User>>::validate_json(value)
    }
}

impl UserUpdateSerializer {
    pub fn fields() -> &'static [Field] {
        <Self as Serializer<User>>::fields()
    }

    pub fn validate_json(value: Value) -> Result<Map<String, Value>> {
        <Self as Serializer<User>>::validate_json(value)
    }
}

fn default_true() -> Value {
    Value::Bool(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn serializes_user_to_json() {
        let user = User {
            id: 1,
            email: "alice@example.com".to_string(),
            name: "Alice".to_string(),
            is_active: true,
        };

        let json = UserPublicSerializer::to_json(&user);

        assert_eq!(json["email"], json!("alice@example.com"));
        assert_eq!(json["name"], json!("Alice"));
    }

    #[test]
    fn validates_create_json_with_default() {
        let payload = json!({
            "email": "alice@example.com",
            "name": "Alice"
        });

        let create = UserCreateSerializer::validate_json(payload).unwrap();

        assert_eq!(create["email"], json!("alice@example.com"));
        assert_eq!(create["name"], json!("Alice"));
        assert_eq!(create["is_active"], json!(true));
    }

    #[test]
    fn rejects_unknown_field() {
        let payload = json!({
            "email": "alice@example.com",
            "name": "Alice",
            "unknown": 1
        });

        let err = UserCreateSerializer::validate_json(payload).unwrap_err();
        assert!(matches!(err, SerializerError::UnknownField(field) if field == "unknown"));
    }
}
