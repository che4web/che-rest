use std::{fs, path::PathBuf, time::SystemTime};

use che_orm::{Model, ModelSerializer, SchemaSet, Uuid};
use che_rest::{AppModule, AppState, CrudViewSet, InstalledApps, ModuleContext};

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(name: &str) -> Self {
        let suffix = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "che_rest_orm03_{name}_{}_{suffix}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Debug, Model)]
#[orm(table = "products")]
struct Product {
    #[orm(primary_key)]
    id: i64,
    price: f64,
    #[orm(db_column = "discount_value")]
    discount: Option<f64>,
    external_id: Uuid,
    #[orm(db_column = "tracking_key")]
    tracking_id: Option<che_orm::Uuid>,
}

#[derive(ModelSerializer)]
#[serializer(model = Product)]
struct ProductSerializer {
    #[serializer(read_only)]
    id: i64,
    price: f64,
    discount: Option<f64>,
    external_id: Uuid,
    tracking_id: Option<che_orm::Uuid>,
}

struct Products;

impl AppModule for Products {
    fn name(&self) -> &'static str {
        "products"
    }

    fn schema(&self) -> SchemaSet {
        SchemaSet::new().model::<Product>()
    }

    fn init(&self, context: &mut ModuleContext) {
        context.viewset(CrudViewSet::<Product, ProductSerializer>::new("/products"));
    }
}

#[tokio::test]
async fn memory_config_uses_one_shared_connection() {
    let directory = TestDirectory::new("memory");
    let config_path = directory.0.join("app.toml");
    for url in [
        ":memory:",
        "sqlite://:memory:",
        "sqlite://:memory:?mode=memory",
    ] {
        for pool_setting in ["", "max_connections = 8\n"] {
            fs::write(
                &config_path,
                format!("[database]\nurl = {url:?}\n{pool_setting}"),
            )
            .unwrap();
            let state = AppState::from_config_file(&config_path).await.unwrap();
            state.database().create_table::<Product>().await.unwrap();
            let (first, second) = tokio::join!(
                state.database().count::<Product>(),
                state.database().count::<Product>()
            );
            assert_eq!(first.unwrap(), 0);
            assert_eq!(second.unwrap(), 0);
        }
    }
}

#[tokio::test]
async fn generators_preserve_real_and_uuid_types_from_model_metadata() {
    let directory = TestDirectory::new("generators");
    let state = AppState::from_database(che_orm::Database::connect_in_memory().await.unwrap());
    let endpoints = InstalledApps::new().add(Products).api_endpoints(state);
    let mut endpoints = endpoints;
    endpoints[0].filters = vec![
        che_rest::module::ApiFilter {
            name: "external_id",
            source: "external_id",
            lookup: che_rest::Lookup::Exact,
        },
        che_rest::module::ApiFilter {
            name: "tracking_id",
            source: "tracking_key",
            lookup: che_rest::Lookup::Exact,
        },
        che_rest::module::ApiFilter {
            name: "price",
            source: "price",
            lookup: che_rest::Lookup::Gte,
        },
    ];
    let client = directory.0.join("client");
    let admin = directory.0.join("admin");
    che_rest::generate_ts::generate(&client, &endpoints, &[], false).unwrap();
    che_rest::generate_admin::generate(&admin, &endpoints, false).unwrap();

    let models = fs::read_to_string(client.join("models.ts")).unwrap();
    assert!(models.contains("external_id?: string;"));
    assert!(models.contains("tracking_id?: string;"));
    assert!(models.contains("price__gte?: number;"));
    assert!(models.contains("export interface Product {\n  id: number;\n  price: number;\n  discount: number | null;\n  external_id: string;\n  tracking_id: string | null;\n}"));
    assert!(models.contains("export interface ProductCreate {\n  price: number;\n  discount?: number | null;\n  external_id: string;\n  tracking_id?: string | null;\n}"));
    assert!(models.contains("export interface ProductUpdate {\n  price?: number;\n  discount?: number | null;\n  external_id?: string;\n  tracking_id?: string | null;\n}"));

    let schema = fs::read_to_string(admin.join("src/admin/generated/adminSchema.ts")).unwrap();
    for (name, ty) in [
        ("external_id", "text"),
        ("tracking_id", "text"),
        ("price__gte", "real"),
    ] {
        let filter = schema
            .lines()
            .find(|line| {
                line.contains(&format!("name: \"{name}\""))
                    && line.contains("label:")
                    && !line.contains("readOnly:")
            })
            .unwrap();
        assert!(filter.contains(&format!("type: \"{ty}\"")), "{filter}");
    }
    for (name, field_type, nullable) in [
        ("price", "real", false),
        ("discount", "real", true),
        ("external_id", "text", false),
        ("tracking_id", "text", true),
    ] {
        let field = schema
            .lines()
            .find(|line| line.contains(&format!("name: \"{name}\", source:")))
            .unwrap();
        assert!(field.contains(&format!("type: \"{field_type}\"")));
        assert!(field.contains(&format!("nullable: {nullable}")));
    }
}
