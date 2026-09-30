//! File-backed image fields. Model columns store only relative keys; application
//! code retains control over who may upload or clear a particular model field.
use std::{
    any::Any, collections::HashMap, fs, future::Future, io::Cursor, path::PathBuf, pin::Pin,
};

use axum::{
    Extension, Router,
    body::{Body, Bytes},
    extract::{Multipart, Path as AxumPath, multipart::Field},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use che_orm::{ImageField, Model, ModelField, ValidatedWrite};
use image::{GenericImageView, ImageFormat, ImageReader, Limits};
use rand::RngCore;

use crate::{
    ApiSchema, ApiSchemaDefinition, AppError, AppModule, AppResult, AppState, CurrentPrincipal,
    ExtensionContext, ModuleContext, MutationHook, OperationSpec, Permission, RestQuerySet,
    ViewAction, ViewSet, ViewSetConfig, ViewSetExtension,
};

#[derive(Clone, Debug)]
pub struct ImagePolicy {
    pub directory: &'static str,
    pub max_bytes: usize,
    pub max_dimension: u32,
    /// Optionally produce a bounded PNG instead of retaining the input bytes.
    pub thumbnail: Option<u32>,
}

impl ImagePolicy {
    pub const fn new(directory: &'static str, max_bytes: usize, max_dimension: u32) -> Self {
        Self {
            directory,
            max_bytes,
            max_dimension,
            thumbnail: None,
        }
    }

    pub const fn thumbnail(mut self, max_dimension: u32) -> Self {
        self.thumbnail = Some(max_dimension);
        self
    }
}

pub struct ImageStorage {
    root: PathBuf,
}

/// Read a multipart image field with an explicit per-field limit. Callers own
/// authorization and any other fields in the multipart form.
pub async fn read_image_field(field: Field<'_>, max_bytes: usize) -> AppResult<axum::body::Bytes> {
    let bytes = field
        .bytes()
        .await
        .map_err(|error| AppError::BadRequest(error.to_string()))?;
    if bytes.is_empty() || bytes.len() > max_bytes {
        return Err(AppError::BadRequest(
            "image exceeds the allowed size".into(),
        ));
    }
    Ok(bytes)
}

pub type ImageGuard = Box<dyn Any + Send>;
pub type ImageFuture<'a, T> = Pin<Box<dyn Future<Output = AppResult<T>> + Send + 'a>>;

/// File plus application-owned multipart data (for example, a scene version).
pub struct ImageForm {
    pub file: Option<Bytes>,
    pub file_name: Option<String>,
    pub content_type: Option<String>,
    pub fields: HashMap<String, Bytes>,
}

impl ImageForm {
    pub fn field(&self, name: &str) -> Option<&[u8]> {
        self.fields.get(name).map(Bytes::as_ref)
    }

    pub fn text(&self, name: &str) -> AppResult<Option<&str>> {
        self.field(name)
            .map(|bytes| {
                std::str::from_utf8(bytes)
                    .map_err(|_| AppError::BadRequest(format!("invalid {name}")))
            })
            .transpose()
    }
}

async fn parse_image_form(mut multipart: Multipart, max_bytes: usize) -> AppResult<ImageForm> {
    let mut form = ImageForm {
        file: None,
        file_name: None,
        content_type: None,
        fields: HashMap::new(),
    };
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| AppError::BadRequest(error.to_string()))?
    {
        let name = field
            .name()
            .ok_or_else(|| AppError::BadRequest("unnamed multipart field".into()))?
            .to_owned();
        if name == "file" {
            if form.file.is_some() {
                return Err(AppError::BadRequest("duplicate file".into()));
            }
            form.file_name = field.file_name().map(str::to_owned);
            form.content_type = field.content_type().map(str::to_owned);
            form.file = Some(read_image_field(field, max_bytes).await?);
        } else {
            let bytes = field
                .bytes()
                .await
                .map_err(|error| AppError::BadRequest(error.to_string()))?;
            if bytes.len() > 64 * 1024 || form.fields.insert(name, bytes).is_some() {
                return Err(AppError::BadRequest("invalid multipart field".into()));
            }
        }
    }
    Ok(form)
}

pub type ImageAuthorize<V> =
    fn(&AppState, Option<&CurrentPrincipal>, &<V as ViewSet>::Model, ViewAction) -> AppResult<()>;
pub type ImageBefore<V> = for<'a> fn(
    &'a AppState,
    &'a <V as ViewSet>::Model,
    &'a ImageForm,
) -> ImageFuture<'a, ImageGuard>;
pub type ImageWrite<V> = fn(
    &AppState,
    &<V as ViewSet>::Model,
    Option<&ImageField>,
    usize,
    ValidatedWrite<<V as ViewSet>::Model>,
) -> AppResult<ValidatedWrite<<V as ViewSet>::Model>>;

/// Declares automatic upload/replace and clear actions for a nullable image
/// field of an existing ViewSet model.
pub struct ImageFieldEndpoint<V: ViewSet> {
    pub name: &'static str,
    pub column: ModelField<V::Model, Option<ImageField>>,
    pub image: for<'a> fn(&'a V::Model) -> Option<&'a ImageField>,
    pub policy: ImagePolicy,
    pub authorize: Option<ImageAuthorize<V>>,
    pub before: Option<ImageBefore<V>>,
    pub write: Option<ImageWrite<V>>,
}

impl<V: ViewSet> Clone for ImageFieldEndpoint<V> {
    fn clone(&self) -> Self {
        Self {
            name: self.name,
            column: self.field(),
            image: self.image,
            policy: self.policy.clone(),
            authorize: self.authorize,
            before: self.before,
            write: self.write,
        }
    }
}

impl<V: ViewSet> ImageFieldEndpoint<V> {
    fn field(&self) -> ModelField<V::Model, Option<ImageField>> {
        let column = self.column.column();
        ModelField::new(column.table, column.name)
    }
    pub fn new(
        name: &'static str,
        column: ModelField<V::Model, Option<ImageField>>,
        image: for<'a> fn(&'a V::Model) -> Option<&'a ImageField>,
        policy: ImagePolicy,
    ) -> Self {
        Self {
            name,
            column,
            image,
            policy,
            authorize: None,
            before: None,
            write: None,
        }
    }

    pub fn authorize(mut self, callback: ImageAuthorize<V>) -> Self {
        self.authorize = Some(callback);
        self
    }
    pub fn before(mut self, callback: ImageBefore<V>) -> Self {
        self.before = Some(callback);
        self
    }
    pub fn write(mut self, callback: ImageWrite<V>) -> Self {
        self.write = Some(callback);
        self
    }

    async fn model(
        &self,
        state: &AppState,
        viewset: &V,
        principal: Option<&CurrentPrincipal>,
        id: i64,
    ) -> AppResult<V::Model> {
        if self.authorize.is_none() {
            V::Permission::default().check(state, principal, ViewAction::Update)?;
        }
        {
            let item = viewset
                .get_queryset()
                .filter(V::Model::primary_key().eq(id))
                .first(state.database())
                .await?
                .ok_or(AppError::NotFound)?;
            if self.authorize.is_none() {
                V::Permission::default().check_object(
                    state,
                    principal,
                    ViewAction::Update,
                    V::QuerySet::item_model(&item),
                )?;
            }
        }
        let model = state
            .database()
            .get::<V::Model>(id)
            .await?
            .ok_or(AppError::NotFound)?;
        if let Some(authorize) = self.authorize {
            authorize(state, principal, &model, ViewAction::Update)?;
        }
        Ok(model)
    }

    async fn replace(
        &self,
        state: AppState,
        viewset: V,
        principal: Option<CurrentPrincipal>,
        id: i64,
        multipart: Multipart,
    ) -> AppResult<axum::Json<ImageUploadResponse>> {
        let form = parse_image_form(multipart, self.policy.max_bytes).await?;
        let bytes = form
            .file
            .as_ref()
            .ok_or_else(|| AppError::BadRequest("file is required".into()))?;
        let model = self.model(&state, &viewset, principal.as_ref(), id).await?;
        let _guard = match self.before {
            Some(callback) => callback(&state, &model, &form).await?,
            None => Box::new(()),
        };
        let image = ImageStorage::from_state(&state)
            .replace_with(
                &state,
                id,
                self.field(),
                (self.image)(&model),
                bytes,
                &self.policy,
                |image, write| match self.write {
                    Some(callback) => callback(&state, &model, Some(image), bytes.len(), write),
                    None => Ok(write),
                },
            )
            .await?;
        Ok(axum::Json(ImageUploadResponse {
            url: url(&image, &state.config.server.api_prefix),
            key: image.key().to_owned(),
        }))
    }

    async fn clear(
        &self,
        state: AppState,
        viewset: V,
        principal: Option<CurrentPrincipal>,
        id: i64,
        multipart: Multipart,
    ) -> AppResult<StatusCode> {
        let form = parse_image_form(multipart, self.policy.max_bytes).await?;
        if form.file.is_some() {
            return Err(AppError::BadRequest(
                "file is not allowed when clearing an image".into(),
            ));
        }
        let model = self.model(&state, &viewset, principal.as_ref(), id).await?;
        let _guard = match self.before {
            Some(callback) => callback(&state, &model, &form).await?,
            None => Box::new(()),
        };
        ImageStorage::from_state(&state)
            .clear_with(
                &state,
                id,
                self.field(),
                (self.image)(&model),
                |write| match self.write {
                    Some(callback) => callback(&state, &model, None, 0, write),
                    None => Ok(write),
                },
            )
            .await?;
        Ok(StatusCode::NO_CONTENT)
    }
}

#[derive(serde::Serialize)]
pub struct ImageUploadResponse {
    pub key: String,
    pub url: String,
}

impl ApiSchema for ImageUploadResponse {
    fn api_schema() -> ApiSchemaDefinition {
        ApiSchemaDefinition {
            name: "ImageUploadResponse",
            openapi: serde_json::json!({"type":"object", "properties":{"key":{"type":"string"}, "url":{"type":"string"}}, "required":["key","url"]}),
            typescript: "export interface ImageUploadResponse { key: string; url: string; }",
        }
    }
}

impl<V: ViewSet> ViewSetExtension<V> for ImageFieldEndpoint<V> {
    const ID: &'static str = "image-field";

    fn install(self, context: &mut ExtensionContext<'_, V>) -> AppResult<()> {
        let endpoint = std::sync::Arc::new(self);
        let path = format!("{{id}}/images/{}", endpoint.name);
        let operation = format!("{}.image.{}", context_id::<V>(), endpoint.name);
        context.route(&path, |route| {
            let upload = endpoint.clone();
            route.put(
                move |Extension(state): Extension<AppState>,
                      Extension(viewset): Extension<V>,
                      who: Option<Extension<CurrentPrincipal>>,
                      AxumPath(id): AxumPath<i64>,
                      form: Multipart| {
                    let endpoint = upload.clone();
                    async move {
                        endpoint
                            .replace(state, viewset, who.map(|value| value.0), id, form)
                            .await
                    }
                },
                OperationSpec::new(format!("{operation}.replace"))
                    .authenticated()
                    .multipart()
                    .body_limit(endpoint.policy.max_bytes + 128 * 1024)
                    .response::<ImageUploadResponse>(StatusCode::OK)
                    .client(endpoint.name, "replace"),
            )?;
            let clear = endpoint.clone();
            route.delete(
                move |Extension(state): Extension<AppState>,
                      Extension(viewset): Extension<V>,
                      who: Option<Extension<CurrentPrincipal>>,
                      AxumPath(id): AxumPath<i64>,
                      form: Multipart| {
                    let endpoint = clear.clone();
                    async move {
                        endpoint
                            .clear(state, viewset, who.map(|value| value.0), id, form)
                            .await
                    }
                },
                OperationSpec::new(format!("{operation}.clear"))
                    .authenticated()
                    .multipart()
                    .body_limit(128 * 1024)
                    .client(endpoint.name, "clear"),
            )
        })
    }
}

fn context_id<V: ViewSet>() -> &'static str {
    std::any::type_name::<V>()
}

pub type ImageCreateAuthorize =
    fn(&AppState, Option<&CurrentPrincipal>, &ImageForm) -> AppResult<()>;
pub type ImageCreateAction<R> = for<'a> fn(
    &'a AppState,
    Option<&'a CurrentPrincipal>,
    &'a ImageField,
    &'a ImageForm,
) -> ImageFuture<'a, R>;

/// Generates a multipart image creation endpoint for an application-owned
/// image record and any associated links. The callback owns domain writes;
/// failed callbacks cause the newly saved file to be removed.
pub struct ImageCreateEndpoint<V: ViewSet, R: ApiSchema + serde::Serialize + Send + Sync + 'static>
{
    pub policy: ImagePolicy,
    pub authorize: ImageCreateAuthorize,
    pub create: ImageCreateAction<R>,
    pub namespace: &'static str,
    pub method: &'static str,
    _marker: std::marker::PhantomData<fn() -> V>,
}

impl<V: ViewSet, R: ApiSchema + serde::Serialize + Send + Sync + 'static> Clone
    for ImageCreateEndpoint<V, R>
{
    fn clone(&self) -> Self {
        Self {
            policy: self.policy.clone(),
            authorize: self.authorize,
            create: self.create,
            namespace: self.namespace,
            method: self.method,
            _marker: std::marker::PhantomData,
        }
    }
}

impl<V: ViewSet, R: ApiSchema + serde::Serialize + Send + Sync + 'static>
    ImageCreateEndpoint<V, R>
{
    pub fn new(
        policy: ImagePolicy,
        authorize: ImageCreateAuthorize,
        create: ImageCreateAction<R>,
        namespace: &'static str,
        method: &'static str,
    ) -> Self {
        Self {
            policy,
            authorize,
            create,
            namespace,
            method,
            _marker: std::marker::PhantomData,
        }
    }

    async fn upload(
        &self,
        state: AppState,
        who: Option<CurrentPrincipal>,
        multipart: Multipart,
    ) -> AppResult<(StatusCode, axum::Json<R>)> {
        let form = parse_image_form(multipart, self.policy.max_bytes).await?;
        (self.authorize)(&state, who.as_ref(), &form)?;
        let bytes = form
            .file
            .as_ref()
            .ok_or_else(|| AppError::BadRequest("file is required".into()))?;
        let storage = ImageStorage::from_state(&state);
        let image = storage.save(bytes, &self.policy)?;
        match (self.create)(&state, who.as_ref(), &image, &form).await {
            Ok(value) => Ok((StatusCode::CREATED, axum::Json(value))),
            Err(error) => {
                let _ = storage.remove(&image);
                Err(error)
            }
        }
    }
}

impl<V: ViewSet, R: ApiSchema + serde::Serialize + Send + Sync + 'static> ViewSetExtension<V>
    for ImageCreateEndpoint<V, R>
{
    const ID: &'static str = "image-create";

    fn install(self, context: &mut ExtensionContext<'_, V>) -> AppResult<()> {
        let endpoint = std::sync::Arc::new(self);
        context.route("upload", |route| {
            let upload = endpoint.clone();
            route.post(
                move |Extension(state): Extension<AppState>,
                      who: Option<Extension<CurrentPrincipal>>,
                      form: Multipart| {
                    let endpoint = upload.clone();
                    async move { endpoint.upload(state, who.map(|value| value.0), form).await }
                },
                OperationSpec::new(format!("{}.image.create", context_id::<V>()))
                    .authenticated()
                    .multipart()
                    .body_limit(endpoint.policy.max_bytes + 128 * 1024)
                    .response::<R>(StatusCode::CREATED)
                    .client(endpoint.namespace, endpoint.method),
            )
        })
    }
}

impl<V: ViewSet> ViewSetConfig<V> {
    pub fn image_field(&mut self, field: ImageFieldEndpoint<V>) -> AppResult<()> {
        self.extend(field)
    }
}

impl ImageStorage {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn from_state(state: &AppState) -> Self {
        Self::new(&state.config.media.root)
    }

    /// Store and attach a replacement only after validation succeeds. If the
    /// database write fails, discard the new file and leave the old one intact.
    pub async fn replace<M: Model + Send + 'static>(
        &self,
        state: &AppState,
        id: i64,
        column: ModelField<M, Option<ImageField>>,
        previous: Option<&ImageField>,
        bytes: &[u8],
        policy: &ImagePolicy,
    ) -> AppResult<ImageField> {
        self.replace_with(
            state,
            id,
            column,
            previous,
            bytes,
            policy,
            |_image, write| Ok(write),
        )
        .await
    }

    /// Compare-and-swap the image key and optional server-owned metadata in one
    /// database update. A losing request removes its own file, not the winner's.
    pub async fn replace_with<M, F>(
        &self,
        state: &AppState,
        id: i64,
        column: ModelField<M, Option<ImageField>>,
        previous: Option<&ImageField>,
        bytes: &[u8],
        policy: &ImagePolicy,
        prepare: F,
    ) -> AppResult<ImageField>
    where
        M: Model + Send + 'static,
        F: FnOnce(&ImageField, ValidatedWrite<M>) -> AppResult<ValidatedWrite<M>>,
    {
        let next = self.save(bytes, policy)?;
        let result = async {
            let reference = column.column();
            let condition: che_orm::Expr = match previous {
                Some(image) => {
                    ModelField::<M, Option<ImageField>>::new(reference.table, reference.name)
                        .eq(Some(image.clone()))
                }
                None => ModelField::<M, Option<ImageField>>::new(reference.table, reference.name)
                    .is_null(),
            };
            let write = ValidatedWrite::Update(
                M::update()
                    .filter(M::primary_key().eq(id))
                    .filter(condition),
            )
            .set(column, Some(next.clone()));
            if prepare(&next, write)?
                .save(state.database())
                .await?
                .is_none()
            {
                return Err(image_update_conflict::<M>(state, id).await?);
            }
            Ok::<_, AppError>(())
        }
        .await;
        if let Err(error) = result {
            let _ = self.remove(&next);
            return Err(error);
        }
        if let Some(old) = previous {
            // The database is already committed. Defer failed file cleanup
            // rather than reporting a failed replacement to the client.
            let _ = self.remove(old);
        }
        Ok(next)
    }

    pub async fn clear<M: Model + Send + 'static>(
        &self,
        state: &AppState,
        id: i64,
        column: ModelField<M, Option<ImageField>>,
        previous: Option<&ImageField>,
    ) -> AppResult<()> {
        self.clear_with(state, id, column, previous, Ok).await
    }

    pub async fn clear_with<M, F>(
        &self,
        state: &AppState,
        id: i64,
        column: ModelField<M, Option<ImageField>>,
        previous: Option<&ImageField>,
        prepare: F,
    ) -> AppResult<()>
    where
        M: Model + Send + 'static,
        F: FnOnce(ValidatedWrite<M>) -> AppResult<ValidatedWrite<M>>,
    {
        let reference = column.column();
        let condition: che_orm::Expr = match previous {
            Some(image) => {
                ModelField::<M, Option<ImageField>>::new(reference.table, reference.name)
                    .eq(Some(image.clone()))
            }
            None => {
                ModelField::<M, Option<ImageField>>::new(reference.table, reference.name).is_null()
            }
        };
        if prepare(
            ValidatedWrite::Update(
                M::update()
                    .filter(M::primary_key().eq(id))
                    .filter(condition),
            )
            .set(column, None::<ImageField>),
        )?
        .save(state.database())
        .await?
        .is_none()
        {
            return Err(image_update_conflict::<M>(state, id).await?);
        }
        if let Some(old) = previous {
            let _ = self.remove(old);
        }
        Ok(())
    }

    pub fn save(&self, bytes: &[u8], policy: &ImagePolicy) -> AppResult<ImageField> {
        if bytes.is_empty() || bytes.len() > policy.max_bytes || !safe_key(policy.directory) {
            return Err(AppError::BadRequest(
                "invalid image size or directory".into(),
            ));
        }
        let format = image::guess_format(bytes)
            .map_err(|_| AppError::BadRequest("unsupported image".into()))?;
        if !matches!(
            format,
            ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::WebP
        ) {
            return Err(AppError::BadRequest("unsupported image format".into()));
        }
        let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
        let mut limits = Limits::default();
        limits.max_image_width = Some(policy.max_dimension);
        limits.max_image_height = Some(policy.max_dimension);
        limits.max_alloc = Some(256 * 1024 * 1024);
        reader.limits(limits);
        let decoded = reader
            .decode()
            .map_err(|_| AppError::BadRequest("invalid image".into()))?;
        let (width, height) = decoded.dimensions();
        if width == 0 || height == 0 {
            return Err(AppError::BadRequest("empty image".into()));
        }
        let (contents, extension) = if let Some(maximum) = policy.thumbnail {
            if maximum == 0 {
                return Err(AppError::BadRequest("invalid thumbnail size".into()));
            }
            let resized = decoded.thumbnail(maximum, maximum);
            let mut output = Cursor::new(Vec::new());
            resized
                .write_to(&mut output, ImageFormat::Png)
                .map_err(|_| AppError::BadRequest("could not encode image".into()))?;
            (output.into_inner(), "png")
        } else {
            (
                bytes.to_vec(),
                match format {
                    ImageFormat::Png => "png",
                    ImageFormat::Jpeg => "jpg",
                    ImageFormat::Gif => "gif",
                    ImageFormat::WebP => "webp",
                    _ => unreachable!(),
                },
            )
        };
        if contents.len() > policy.max_bytes {
            return Err(AppError::BadRequest("processed image is too large".into()));
        }
        let directory = self.root.join(policy.directory);
        fs::create_dir_all(&directory).map_err(AppError::Io)?;
        let mut random = [0_u8; 16];
        rand::thread_rng().fill_bytes(&mut random);
        let name = format!("{}.{extension}", hex::encode(random));
        let key = format!("{}/{}", policy.directory, name);
        let path = self.root.join(&key);
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(AppError::Io)?;
        use std::io::Write;
        if let Err(error) = file.write_all(&contents) {
            drop(file);
            let _ = fs::remove_file(path);
            return Err(AppError::Io(error));
        }
        if let Err(error) = self.retry_pending_cleanup() {
            eprintln!("image cleanup retry failed: {error}");
        }
        Ok(ImageField::new(key))
    }

    pub fn remove(&self, image: &ImageField) -> AppResult<()> {
        let path = self.path(image)?;
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => {
                if let Err(queue_error) = self.queue_cleanup(image) {
                    eprintln!(
                        "could not queue image cleanup for {}: {queue_error}",
                        image.key()
                    );
                }
                Err(AppError::Io(error))
            }
        }
    }

    fn cleanup_queue(&self) -> PathBuf {
        self.root.join(".pending-image-deletions")
    }

    fn queue_cleanup(&self, image: &ImageField) -> std::io::Result<()> {
        let directory = self.cleanup_queue();
        fs::create_dir_all(&directory)?;
        let mut random = [0_u8; 16];
        rand::thread_rng().fill_bytes(&mut random);
        let identifier = hex::encode(random);
        let temporary = directory.join(format!("{identifier}.tmp"));
        let final_path = directory.join(format!("{identifier}.key"));
        let result = (|| {
            use std::io::Write;
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(image.key().as_bytes())?;
            file.sync_all()?;
            fs::rename(&temporary, &final_path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    /// Retry removals which failed after the corresponding database mutation.
    pub fn retry_pending_cleanup(&self) -> AppResult<usize> {
        let entries = match fs::read_dir(self.cleanup_queue()) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(error) => return Err(AppError::Io(error)),
        };
        let mut completed = 0;
        for entry in entries {
            let entry = entry.map_err(AppError::Io)?;
            if entry
                .path()
                .extension()
                .is_none_or(|extension| extension != "key")
            {
                continue;
            }
            let key = fs::read_to_string(entry.path()).map_err(AppError::Io)?;
            let image = ImageField::new(key);
            let path = match self.path(&image) {
                Ok(path) => path,
                Err(error) => {
                    eprintln!(
                        "invalid pending image cleanup entry {}: {error}",
                        entry.path().display()
                    );
                    continue;
                }
            };
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    eprintln!("pending image cleanup for {} failed: {error}", image.key());
                    continue;
                }
            }
            match fs::remove_file(entry.path()) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(AppError::Io(error)),
            }
            completed += 1;
        }
        Ok(completed)
    }

    pub fn read(&self, image: &ImageField) -> AppResult<(Vec<u8>, &'static str)> {
        let path = self.path(image)?;
        let media_type = match path.extension().and_then(|value| value.to_str()) {
            Some("png") => "image/png",
            Some("jpg") => "image/jpeg",
            Some("gif") => "image/gif",
            Some("webp") => "image/webp",
            _ => return Err(AppError::BadRequest("unsupported image extension".into())),
        };
        Ok((fs::read(path).map_err(AppError::Io)?, media_type))
    }

    fn path(&self, image: &ImageField) -> AppResult<PathBuf> {
        if !safe_key(image.key()) {
            return Err(AppError::BadRequest("invalid image key".into()));
        }
        Ok(self.root.join(image.key()))
    }
}

async fn image_update_conflict<M: Model + Send + 'static>(
    state: &AppState,
    id: i64,
) -> AppResult<AppError> {
    Ok(if state.database().get::<M>(id).await?.is_some() {
        AppError::Conflict("image has changed".into())
    } else {
        AppError::NotFound
    })
}

/// Cleans up an image after a ViewSet deletes its owning row.
pub struct ImageCleanup<V: ViewSet> {
    pub id: &'static str,
    pub image: for<'a> fn(&'a V::Model) -> Option<&'a ImageField>,
}

impl<V: ViewSet> MutationHook<V> for ImageCleanup<V> {
    fn id(&self) -> &'static str {
        self.id
    }

    fn prepare_delete(&self, model: &V::Model) -> serde_json::Value {
        (self.image)(model)
            .map(|image| serde_json::json!(image.key()))
            .unwrap_or(serde_json::Value::Null)
    }

    fn after_delete(&self, state: &AppState, payload: &serde_json::Value) -> AppResult<()> {
        if let Some(key) = payload.as_str() {
            ImageStorage::from_state(state).remove(&ImageField::new(key))?;
        }
        Ok(())
    }
}

pub fn url(image: &ImageField, api_prefix: &str) -> String {
    format!("{}/media/{}", api_prefix.trim_end_matches('/'), image.key())
}

fn safe_key(key: &str) -> bool {
    !key.is_empty()
        && key.split('/').all(|segment| {
            !segment.is_empty()
                && segment != "."
                && segment != ".."
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        })
}

pub struct MediaModule;
pub fn module() -> MediaModule {
    MediaModule
}
impl AppModule for MediaModule {
    fn name(&self) -> &'static str {
        "media"
    }
    fn schema(&self) -> che_orm::SchemaSet {
        che_orm::SchemaSet::new()
    }
    fn init(&self, context: &mut ModuleContext) {
        context.route(Router::new().route("/media/{*key}", get(download_image)));
    }

    fn start(&self, state: &AppState) {
        if let Err(error) = ImageStorage::from_state(state).retry_pending_cleanup() {
            eprintln!("image cleanup retry failed on startup: {error}");
        }
    }
}

async fn download_image(
    Extension(state): Extension<AppState>,
    current: Option<Extension<CurrentPrincipal>>,
    AxumPath(key): AxumPath<String>,
) -> Response {
    if current.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Ok((bytes, media_type)) = ImageStorage::from_state(&state).read(&ImageField::new(key))
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static(media_type),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        header::HeaderValue::from_static("nosniff"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("private, max-age=60"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use che_orm::{Database, DatabaseQuery, Model, ModelSerializer};
    use tower::ServiceExt;

    #[derive(Debug, Model)]
    #[orm(table = "image_cleanup_items")]
    struct ImageItem {
        #[orm(primary_key)]
        id: i64,
        image: Option<ImageField>,
    }

    #[derive(Debug, Model)]
    #[orm(table = "image_metadata_items")]
    struct ImageMetadataItem {
        #[orm(primary_key)]
        id: i64,
        image: Option<ImageField>,
        url: Option<String>,
    }

    #[derive(ModelSerializer)]
    #[serializer(model = ImageItem)]
    struct ImageItemSerializer {
        #[serializer(read_only)]
        id: i64,
        #[serializer(read_only)]
        image: Option<ImageField>,
    }

    #[derive(Clone)]
    struct ImageViewSet;

    impl ViewSet for ImageViewSet {
        type Model = ImageItem;
        type Serializer = ImageItemSerializer;
        type ListSerializer = Self::Serializer;
        type QuerySet = DatabaseQuery<ImageItem>;
        type FilterSet = crate::FilterSet<ImageItem>;
        type Permission = crate::AllowAny;

        fn get_queryset(&self) -> Self::QuerySet {
            DatabaseQuery::new(ImageItem::query())
        }
        fn path(&self) -> &'static str {
            "/image-items"
        }
        fn signal_access(&self, action: ViewAction) -> Option<crate::SignalAccess> {
            (action == ViewAction::Delete).then_some(crate::SignalAccess::Authenticated)
        }
    }

    #[test]
    fn image_field_validates_resizes_and_stays_inside_storage_root() {
        let root = std::env::temp_dir().join(format!("che-rest-media-{}", rand::random::<u64>()));
        let storage = ImageStorage::new(&root);
        let mut png = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(24, 12)
            .write_to(&mut png, ImageFormat::Png)
            .unwrap();
        let policy = ImagePolicy::new("previews", 1024 * 1024, 100).thumbnail(8);
        let field = storage.save(&png.into_inner(), &policy).unwrap();
        assert!(field.key().starts_with("previews/"));
        let (bytes, media_type) = storage.read(&field).unwrap();
        assert_eq!(media_type, "image/png");
        assert_eq!(
            image::load_from_memory(&bytes).unwrap().dimensions(),
            (8, 4)
        );
        assert!(storage.read(&ImageField::new("../outside.png")).is_err());
        assert!(storage.save(b"not an image", &policy).is_err());
        storage.remove(&field).unwrap();
        assert!(storage.read(&field).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_post_commit_deletion_is_queued_and_retried() {
        let root = std::env::temp_dir().join(format!("che-rest-media-{}", rand::random::<u64>()));
        let storage = ImageStorage::new(&root);
        let path = root.join("avatars/blocked.png");
        fs::create_dir_all(&path).unwrap();
        let key = ImageField::new("avatars/blocked.png");
        assert!(storage.remove(&key).is_err());
        assert_eq!(
            fs::read_dir(root.join(".pending-image-deletions"))
                .unwrap()
                .count(),
            1
        );
        fs::remove_dir(path).unwrap();
        assert_eq!(storage.retry_pending_cleanup().unwrap(), 1);
        assert_eq!(
            fs::read_dir(root.join(".pending-image-deletions"))
                .unwrap()
                .count(),
            0
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn viewset_deletion_removes_owned_image_after_database_delete() {
        let root = std::env::temp_dir().join(format!("che-rest-media-{}", rand::random::<u64>()));
        let mut state = AppState::from_database(Database::connect_in_memory().unwrap());
        state.config.media.root = root.to_string_lossy().into_owned();
        state.database().create_table::<ImageItem>().await.unwrap();
        let mut png = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(1, 1)
            .write_to(&mut png, ImageFormat::Png)
            .unwrap();
        let storage = ImageStorage::from_state(&state);
        let image = storage
            .save(
                &png.into_inner(),
                &ImagePolicy::new("items", 1024 * 1024, 10),
            )
            .unwrap();
        let item = state
            .database()
            .create::<ImageItem>()
            .set(ImageItem::IMAGE, Some(image.clone()))
            .execute()
            .await
            .unwrap();
        let hooks: Vec<std::sync::Arc<dyn MutationHook<ImageViewSet>>> =
            vec![std::sync::Arc::new(ImageCleanup::<ImageViewSet> {
                id: "item-image",
                image: |item| item.image.as_ref(),
            })];
        let app = crate::rest::router::router_with_hooks(
            state.clone(),
            ImageViewSet,
            Router::new(),
            hooks,
        );
        let result = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/image-items/{}/", item.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(result.status(), StatusCode::NO_CONTENT);
        assert!(
            state
                .database()
                .get::<ImageItem>(item.id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(storage.read(&image).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn failed_post_commit_cleanup_does_not_fail_delete_or_suppress_signal() {
        let root = std::env::temp_dir().join(format!("che-rest-media-{}", rand::random::<u64>()));
        let mut state = AppState::from_database(Database::connect_in_memory().unwrap());
        state.config.media.root = root.to_string_lossy().into_owned();
        state.database().create_table::<ImageItem>().await.unwrap();
        let image = ImageField::new("items/blocked.png");
        let blocked = root.join(image.key());
        fs::create_dir_all(&blocked).unwrap();
        let item = state
            .database()
            .create::<ImageItem>()
            .set(ImageItem::IMAGE, Some(image))
            .execute()
            .await
            .unwrap();
        let hooks: Vec<std::sync::Arc<dyn MutationHook<ImageViewSet>>> =
            vec![std::sync::Arc::new(ImageCleanup::<ImageViewSet> {
                id: "item-image",
                image: |item| item.image.as_ref(),
            })];
        let mut receiver = state.signals().subscribe("image-items.deleted");
        let app = crate::rest::router::router_with_hooks(
            state.clone(),
            ImageViewSet,
            Router::new(),
            hooks,
        );
        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/image-items/{}/", item.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert!(
            state
                .database()
                .get::<ImageItem>(item.id)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(receiver.try_recv().unwrap()["id"], item.id);
        assert_eq!(
            fs::read_dir(root.join(".pending-image-deletions"))
                .unwrap()
                .count(),
            1
        );
        fs::remove_dir(blocked).unwrap();
        assert_eq!(
            ImageStorage::from_state(&state)
                .retry_pending_cleanup()
                .unwrap(),
            1
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn replace_discards_image_when_model_disappears() {
        let root = std::env::temp_dir().join(format!("che-rest-media-{}", rand::random::<u64>()));
        let mut state = AppState::from_database(Database::connect_in_memory().unwrap());
        state.config.media.root = root.to_string_lossy().into_owned();
        state.database().create_table::<ImageItem>().await.unwrap();
        let mut png = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(1, 1)
            .write_to(&mut png, ImageFormat::Png)
            .unwrap();
        let result = ImageStorage::from_state(&state)
            .replace::<ImageItem>(
                &state,
                42,
                ImageItem::IMAGE,
                None,
                &png.into_inner(),
                &ImagePolicy::new("items", 1024 * 1024, 10),
            )
            .await;
        assert!(matches!(result, Err(AppError::NotFound)));
        assert_eq!(fs::read_dir(root.join("items")).unwrap().count(), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn stale_replacement_and_clear_cannot_overwrite_winner_or_leave_files() {
        let root = std::env::temp_dir().join(format!("che-rest-media-{}", rand::random::<u64>()));
        let mut state = AppState::from_database(Database::connect_in_memory().unwrap());
        state.config.media.root = root.to_string_lossy().into_owned();
        state.database().create_table::<ImageItem>().await.unwrap();
        let item = state
            .database()
            .create::<ImageItem>()
            .set(ImageItem::IMAGE, None::<ImageField>)
            .execute()
            .await
            .unwrap();
        let storage = ImageStorage::from_state(&state);
        let mut png = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(1, 1)
            .write_to(&mut png, ImageFormat::Png)
            .unwrap();
        let policy = ImagePolicy::new("items", 1024 * 1024, 10);
        let winner = storage
            .replace(
                &state,
                item.id,
                ImageItem::IMAGE,
                None,
                png.get_ref(),
                &policy,
            )
            .await
            .unwrap();
        assert!(matches!(
            storage
                .replace(
                    &state,
                    item.id,
                    ImageItem::IMAGE,
                    None,
                    png.get_ref(),
                    &policy
                )
                .await,
            Err(AppError::Conflict(_))
        ));
        assert!(matches!(
            storage.clear(&state, item.id, ImageItem::IMAGE, None).await,
            Err(AppError::Conflict(_))
        ));
        assert_eq!(
            state
                .database()
                .get::<ImageItem>(item.id)
                .await
                .unwrap()
                .unwrap()
                .image
                .as_ref(),
            Some(&winner)
        );
        assert_eq!(fs::read_dir(root.join("items")).unwrap().count(), 1);
        storage
            .clear(&state, item.id, ImageItem::IMAGE, Some(&winner))
            .await
            .unwrap();
        assert_eq!(fs::read_dir(root.join("items")).unwrap().count(), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn conditional_image_and_metadata_update_commit_together() {
        let root = std::env::temp_dir().join(format!("che-rest-media-{}", rand::random::<u64>()));
        let mut state = AppState::from_database(Database::connect_in_memory().unwrap());
        state.config.media.root = root.to_string_lossy().into_owned();
        state
            .database()
            .create_table::<ImageMetadataItem>()
            .await
            .unwrap();
        let item = state
            .database()
            .create::<ImageMetadataItem>()
            .set(ImageMetadataItem::IMAGE, None::<ImageField>)
            .set(ImageMetadataItem::URL, None::<String>)
            .execute()
            .await
            .unwrap();
        let mut png = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(1, 1)
            .write_to(&mut png, ImageFormat::Png)
            .unwrap();
        let storage = ImageStorage::from_state(&state);
        let policy = ImagePolicy::new("items", 1024 * 1024, 10);
        let write = |image: &ImageField, pending: ValidatedWrite<ImageMetadataItem>| {
            Ok(pending.set(ImageMetadataItem::URL, Some(image.key().to_owned())))
        };
        let winner = storage
            .replace_with(
                &state,
                item.id,
                ImageMetadataItem::IMAGE,
                None,
                png.get_ref(),
                &policy,
                write,
            )
            .await
            .unwrap();
        assert!(matches!(
            storage
                .replace_with(
                    &state,
                    item.id,
                    ImageMetadataItem::IMAGE,
                    None,
                    png.get_ref(),
                    &policy,
                    write
                )
                .await,
            Err(AppError::Conflict(_))
        ));
        let actual = state
            .database()
            .get::<ImageMetadataItem>(item.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(actual.image.as_ref(), Some(&winner));
        assert_eq!(actual.url.as_deref(), Some(winner.key()));
        assert_eq!(fs::read_dir(root.join("items")).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn replacement_succeeds_when_old_file_cleanup_must_be_queued() {
        let root = std::env::temp_dir().join(format!("che-rest-media-{}", rand::random::<u64>()));
        let mut state = AppState::from_database(Database::connect_in_memory().unwrap());
        state.config.media.root = root.to_string_lossy().into_owned();
        state.database().create_table::<ImageItem>().await.unwrap();
        let old = ImageField::new("items/blocked.png");
        let blocked = root.join(old.key());
        fs::create_dir_all(&blocked).unwrap();
        let item = state
            .database()
            .create::<ImageItem>()
            .set(ImageItem::IMAGE, Some(old.clone()))
            .execute()
            .await
            .unwrap();
        let mut png = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(1, 1)
            .write_to(&mut png, ImageFormat::Png)
            .unwrap();
        let storage = ImageStorage::from_state(&state);
        let image = storage
            .replace(
                &state,
                item.id,
                ImageItem::IMAGE,
                Some(&old),
                png.get_ref(),
                &ImagePolicy::new("items", 1024 * 1024, 10),
            )
            .await
            .unwrap();
        assert_eq!(
            state
                .database()
                .get::<ImageItem>(item.id)
                .await
                .unwrap()
                .unwrap()
                .image
                .as_ref(),
            Some(&image)
        );
        assert_eq!(
            fs::read_dir(root.join(".pending-image-deletions"))
                .unwrap()
                .count(),
            1
        );
        fs::remove_dir(blocked).unwrap();
        assert_eq!(storage.retry_pending_cleanup().unwrap(), 1);
        storage.remove(&image).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn nested_media_keys_are_captured_without_a_leading_slash() {
        let router = Router::new().route(
            "/media/{*key}",
            get(|AxumPath(key): AxumPath<String>| async move { key }),
        );
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/media/avatars/photo.png")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(&body[..], b"avatars/photo.png");
    }

    #[tokio::test]
    async fn media_route_requires_authentication() {
        let state = AppState::from_database(Database::connect_in_memory().unwrap());
        let router = Router::new()
            .route("/media/{*key}", get(download_image))
            .layer(Extension(state));
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/media/avatars/photo.png")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn declared_image_field_uploads_replaces_and_clears() {
        let root = std::env::temp_dir().join(format!("che-rest-media-{}", rand::random::<u64>()));
        let mut state = AppState::from_database(Database::connect_in_memory().unwrap());
        state.config.media.root = root.to_string_lossy().into_owned();
        state.database().create_table::<ImageItem>().await.unwrap();
        let item = state
            .database()
            .create::<ImageItem>()
            .set(ImageItem::IMAGE, None::<ImageField>)
            .execute()
            .await
            .unwrap();
        let mut config = ViewSetConfig::<ImageViewSet>::new("/image-items");
        config
            .image_field(ImageFieldEndpoint::<ImageViewSet>::new(
                "photo",
                ImageItem::IMAGE,
                |item: &ImageItem| item.image.as_ref(),
                ImagePolicy::new("photos", 1024 * 1024, 32),
            ))
            .unwrap();
        let (extensions, metadata, _, hooks) = config.into_parts();
        assert_eq!(metadata[0].operations.len(), 2);
        assert_eq!(
            metadata[0].operations[0].path,
            "/image-items/{id}/images/photo/"
        );
        assert!(metadata[0].operations[0].multipart);
        assert_eq!(
            metadata[0].operations[0].openapi["requestBody"]["content"]["multipart/form-data"]["schema"]
                ["required"][0],
            "file"
        );
        assert!(metadata[0].operations[1].openapi["requestBody"]["content"]["multipart/form-data"]["schema"]["properties"].is_null());
        let app =
            crate::rest::router::router_with_hooks(state.clone(), ImageViewSet, extensions, hooks);
        let mut png = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(2, 2)
            .write_to(&mut png, ImageFormat::Png)
            .unwrap();
        let multipart = |file: Option<&[u8]>| {
            let mut data = Vec::new();
            if let Some(file) = file {
                data.extend_from_slice(b"--test\r\nContent-Disposition: form-data; name=\"file\"; filename=\"photo.png\"\r\nContent-Type: image/png\r\n\r\n");
                data.extend_from_slice(file);
                data.extend_from_slice(b"\r\n");
            }
            data.extend_from_slice(b"--test--\r\n");
            data
        };
        let path = format!("/image-items/{}/images/photo/", item.id);
        let request = |method, data| {
            Request::builder()
                .method(method)
                .uri(&path)
                .header("Content-Type", "multipart/form-data; boundary=test")
                .body(Body::from(data))
                .unwrap()
        };
        let response = app
            .clone()
            .oneshot(request("PUT", multipart(Some(&png.get_ref()[..]))))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let first = state
            .database()
            .get::<ImageItem>(item.id)
            .await
            .unwrap()
            .unwrap()
            .image
            .unwrap();
        assert!(ImageStorage::from_state(&state).read(&first).is_ok());
        let response = app
            .clone()
            .oneshot(request("PUT", multipart(Some(&png.get_ref()[..]))))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let next = state
            .database()
            .get::<ImageItem>(item.id)
            .await
            .unwrap()
            .unwrap()
            .image
            .unwrap();
        assert_ne!(next, first);
        assert!(ImageStorage::from_state(&state).read(&first).is_err());
        let response = app
            .oneshot(request("DELETE", multipart(None)))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert!(
            state
                .database()
                .get::<ImageItem>(item.id)
                .await
                .unwrap()
                .unwrap()
                .image
                .is_none()
        );
        assert!(ImageStorage::from_state(&state).read(&next).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[derive(serde::Serialize)]
    struct CreatedImage {
        key: String,
    }

    impl ApiSchema for CreatedImage {
        fn api_schema() -> ApiSchemaDefinition {
            ApiSchemaDefinition {
                name: "CreatedImage",
                openapi: serde_json::json!({"type":"object","properties":{"key":{"type":"string"}},"required":["key"]}),
                typescript: "export interface CreatedImage { key: string; }",
            }
        }
    }

    fn authorize_create(
        _state: &AppState,
        _who: Option<&CurrentPrincipal>,
        form: &ImageForm,
    ) -> AppResult<()> {
        if form.text("target")? != Some("allowed") {
            return Err(AppError::Forbidden("invalid target".into()));
        }
        Ok(())
    }

    fn create_image<'a>(
        _state: &'a AppState,
        _who: Option<&'a CurrentPrincipal>,
        image: &'a ImageField,
        form: &'a ImageForm,
    ) -> ImageFuture<'a, CreatedImage> {
        Box::pin(async move {
            if form.text("fail")? == Some("true") {
                return Err(AppError::BadRequest("invalid link".into()));
            }
            Ok(CreatedImage {
                key: image.key().into(),
            })
        })
    }

    #[tokio::test]
    async fn declared_image_creation_checks_domain_data_and_cleans_failed_uploads() {
        let root = std::env::temp_dir().join(format!("che-rest-media-{}", rand::random::<u64>()));
        let mut state = AppState::from_database(Database::connect_in_memory().unwrap());
        state.config.media.root = root.to_string_lossy().into_owned();
        let mut config = ViewSetConfig::<ImageViewSet>::new("/image-items");
        config
            .extend(ImageCreateEndpoint::<ImageViewSet, CreatedImage>::new(
                ImagePolicy::new("items", 1024 * 1024, 10),
                authorize_create,
                create_image,
                "upload",
                "image",
            ))
            .unwrap();
        let (extensions, operations, _, hooks) = config.into_parts();
        assert_eq!(operations[0].operations[0].path, "/image-items/upload/");
        let app =
            crate::rest::router::router_with_hooks(state.clone(), ImageViewSet, extensions, hooks);
        let mut png = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(1, 1)
            .write_to(&mut png, ImageFormat::Png)
            .unwrap();
        let form = |target: &str, fail: bool| {
            let mut data = format!(
                "--test\r\nContent-Disposition: form-data; name=\"target\"\r\n\r\n{target}\r\n"
            )
            .into_bytes();
            if fail {
                data.extend_from_slice(
                    b"--test\r\nContent-Disposition: form-data; name=\"fail\"\r\n\r\ntrue\r\n",
                );
            }
            data.extend_from_slice(b"--test\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.png\"\r\nContent-Type: image/png\r\n\r\n");
            data.extend_from_slice(png.get_ref());
            data.extend_from_slice(b"\r\n--test--\r\n");
            Request::builder()
                .method("POST")
                .uri("/image-items/upload/")
                .header("Content-Type", "multipart/form-data; boundary=test")
                .body(Body::from(data))
                .unwrap()
        };
        assert_eq!(
            app.clone()
                .oneshot(form("denied", false))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            app.clone()
                .oneshot(form("allowed", true))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(fs::read_dir(root.join("items")).unwrap().count(), 0);
        assert_eq!(
            app.oneshot(form("allowed", false)).await.unwrap().status(),
            StatusCode::CREATED
        );
        assert_eq!(fs::read_dir(root.join("items")).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }
}
