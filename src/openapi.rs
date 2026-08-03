use che_orm::FieldType;
use serde_json::{Map, Value, json};

use crate::{ApiEndpoint, ApiField};

#[derive(Debug, Clone)]
pub struct OpenApiOptions {
    pub title: String,
    pub version: String,
    pub api_prefix: String,
}

impl Default for OpenApiOptions {
    fn default() -> Self {
        Self {
            title: "che-rest API".to_string(),
            version: "0.1.0".to_string(),
            api_prefix: "/api".to_string(),
        }
    }
}

pub fn openapi_json(endpoints: &[ApiEndpoint], options: OpenApiOptions) -> Value {
    let mut paths = Map::new();
    let mut schemas = Map::new();

    schemas.insert("Error".to_string(), error_schema());

    for endpoint in endpoints {
        schemas.insert(endpoint.model_name.clone(), response_schema(endpoint));
        schemas.insert(
            format!("{}Create", endpoint.model_name),
            create_schema(endpoint),
        );
        schemas.insert(
            format!("{}Update", endpoint.model_name),
            update_schema(endpoint),
        );
        schemas.insert(
            format!("{}List", endpoint.model_name),
            list_schema(endpoint),
        );

        paths.insert(
            collection_path(&options.api_prefix, &endpoint.path),
            collection_path_item(endpoint),
        );
        paths.insert(
            detail_path(&options.api_prefix, &endpoint.path),
            detail_path_item(endpoint),
        );
    }

    json!({
        "openapi": "3.0.3",
        "info": {
            "title": options.title,
            "version": options.version,
        },
        "paths": paths,
        "components": {
            "schemas": schemas,
        },
    })
}

pub fn swagger_ui_html(openapi_url: &str, title: &str) -> String {
    format!(
        r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>{title}</title>
    <link rel="stylesheet" href="https://unpkg.com/swagger-ui-dist@5/swagger-ui.css" />
    <style>
      html {{ box-sizing: border-box; overflow-y: scroll; }}
      *, *::before, *::after {{ box-sizing: inherit; }}
      body {{ margin: 0; background: #fafafa; }}
    </style>
  </head>
  <body>
    <div id="swagger-ui"></div>
    <script src="https://unpkg.com/swagger-ui-dist@5/swagger-ui-bundle.js"></script>
    <script>
      window.addEventListener('load', () => {{
        window.ui = SwaggerUIBundle({{
          url: '{openapi_url}',
          dom_id: '#swagger-ui',
          deepLinking: true,
          presets: [SwaggerUIBundle.presets.apis],
          layout: 'BaseLayout'
        }});
      }});
    </script>
  </body>
</html>
"#,
        title = html_escape(title),
        openapi_url = js_string_escape(openapi_url)
    )
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn js_string_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\'', "\\'")
}

fn collection_path(api_prefix: &str, path: &str) -> String {
    let api_prefix = api_prefix.trim_end_matches('/');
    let path = path.trim_matches('/');
    format!("/{api_prefix}/{path}/").replace("//", "/")
}

fn detail_path(api_prefix: &str, path: &str) -> String {
    format!("{}{{id}}/", collection_path(api_prefix, path))
}

fn collection_path_item(endpoint: &ApiEndpoint) -> Value {
    json!({
        "get": {
            "operationId": format!("list{}", endpoint.model_name),
            "tags": [endpoint.resource],
            "parameters": list_parameters(endpoint),
            "responses": {
                "200": json_response_ref(&format!("{}List", endpoint.model_name)),
                "400": error_response(),
            },
        },
        "post": {
            "operationId": format!("create{}", endpoint.model_name),
            "tags": [endpoint.resource],
            "requestBody": json_request_ref(&format!("{}Create", endpoint.model_name), true),
            "responses": {
                "201": json_response_ref(&endpoint.model_name),
                "400": error_response(),
            },
        },
    })
}

fn detail_path_item(endpoint: &ApiEndpoint) -> Value {
    json!({
        "get": {
            "operationId": format!("retrieve{}", endpoint.model_name),
            "tags": [endpoint.resource],
            "parameters": [id_parameter()],
            "responses": {
                "200": json_response_ref(&endpoint.model_name),
                "404": error_response(),
            },
        },
        "patch": {
            "operationId": format!("update{}", endpoint.model_name),
            "tags": [endpoint.resource],
            "parameters": [id_parameter()],
            "requestBody": json_request_ref(&format!("{}Update", endpoint.model_name), true),
            "responses": {
                "200": json_response_ref(&endpoint.model_name),
                "400": error_response(),
                "404": error_response(),
            },
        },
        "delete": {
            "operationId": format!("destroy{}", endpoint.model_name),
            "tags": [endpoint.resource],
            "parameters": [id_parameter()],
            "responses": {
                "204": { "description": "No content" },
                "404": error_response(),
            },
        },
    })
}

fn response_schema(endpoint: &ApiEndpoint) -> Value {
    object_schema(
        endpoint
            .fields
            .iter()
            .filter(|field| !field.write_only)
            .map(|field| (field.name.clone(), response_field_schema(field)))
            .collect(),
        endpoint
            .fields
            .iter()
            .filter(|field| !field.write_only && field.required && !field.has_default)
            .map(|field| field.name.clone())
            .collect(),
    )
}

fn create_schema(endpoint: &ApiEndpoint) -> Value {
    object_schema(
        endpoint
            .fields
            .iter()
            .filter(|field| !field.read_only)
            .map(|field| (field.name.clone(), field_schema(field.ty, field.nullable)))
            .collect(),
        endpoint
            .fields
            .iter()
            .filter(|field| !field.read_only && field.required && !field.has_default)
            .map(|field| field.name.clone())
            .collect(),
    )
}

fn update_schema(endpoint: &ApiEndpoint) -> Value {
    object_schema(
        endpoint
            .fields
            .iter()
            .filter(|field| !field.read_only)
            .map(|field| (field.name.clone(), field_schema(field.ty, field.nullable)))
            .collect(),
        Vec::new(),
    )
}

fn list_schema(endpoint: &ApiEndpoint) -> Value {
    json!({
        "type": "object",
        "required": ["count", "results"],
        "properties": {
            "count": { "type": "integer", "format": "int64" },
            "results": {
                "type": "array",
                "items": schema_ref(&endpoint.model_name),
            },
        },
    })
}

fn object_schema(properties: Vec<(String, Value)>, required: Vec<String>) -> Value {
    let mut property_map = Map::new();
    for (name, schema) in properties {
        property_map.insert(name, schema);
    }

    let mut schema = Map::new();
    schema.insert("type".to_string(), json!("object"));
    schema.insert("properties".to_string(), Value::Object(property_map));
    if !required.is_empty() {
        schema.insert("required".to_string(), json!(required));
    }
    Value::Object(schema)
}

fn response_field_schema(field: &ApiField) -> Value {
    match &field.related_model {
        Some(model) if field.nullable => {
            let mut schema = schema_ref(model);
            schema["nullable"] = json!(true);
            schema
        }
        Some(model) => schema_ref(model),
        None => field_schema(field.ty, field.nullable),
    }
}

fn field_schema(ty: FieldType, nullable: bool) -> Value {
    let mut schema = match ty {
        FieldType::Integer => json!({ "type": "integer", "format": "int64" }),
        FieldType::Text => json!({ "type": "string" }),
        FieldType::Boolean => json!({ "type": "boolean" }),
        FieldType::Real => json!({ "type": "number", "format": "double" }),
        FieldType::DateTime => json!({ "type": "string", "format": "date-time" }),
    };

    if nullable {
        schema["nullable"] = json!(true);
    }

    schema
}

fn list_parameters(endpoint: &ApiEndpoint) -> Vec<Value> {
    let mut parameters = vec![
        query_parameter("limit", json!({ "type": "integer", "format": "int64" })),
        query_parameter("offset", json!({ "type": "integer", "format": "int64" })),
        query_parameter("ordering", json!({ "type": "string" })),
    ];

    parameters.extend(
        endpoint
            .filters
            .iter()
            .map(|filter| query_parameter(&filter.name, field_schema(filter.ty, filter.nullable))),
    );

    parameters
}

fn query_parameter(name: &str, schema: Value) -> Value {
    json!({
        "name": name,
        "in": "query",
        "required": false,
        "schema": schema,
    })
}

fn id_parameter() -> Value {
    json!({
        "name": "id",
        "in": "path",
        "required": true,
        "schema": { "type": "integer", "format": "int64" },
    })
}

fn json_response_ref(schema: &str) -> Value {
    json!({
        "description": "OK",
        "content": {
            "application/json": {
                "schema": schema_ref(schema),
            },
        },
    })
}

fn json_request_ref(schema: &str, required: bool) -> Value {
    json!({
        "required": required,
        "content": {
            "application/json": {
                "schema": schema_ref(schema),
            },
        },
    })
}

fn error_response() -> Value {
    json_response_ref("Error")
}

fn error_schema() -> Value {
    json!({
        "type": "object",
        "required": ["detail"],
        "properties": {
            "detail": { "type": "string" },
        },
    })
}

fn schema_ref(schema: &str) -> Value {
    json!({ "$ref": format!("#/components/schemas/{schema}") })
}
