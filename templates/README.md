# Cargo project template

Install the tested cargo-generate version:

```bash
cargo install cargo-generate --version 0.23.7 --locked
```

Generate from a local checkout (absolute dependency paths are recommended):

```bash
cargo generate --path /home/che/web/rust/che-rest/templates/project \
  --name my-project \
  --define che_rest_path=/home/che/web/rust/che-rest \
  --define che_orm_path=/home/che/web/rust/che-orm \
  --define with_auth=true
```

Once this template is committed and pushed, generation from Git is available:

```bash
cargo generate --git https://github.com/che4web/che-rest \
  --branch feat/rest-offline-sync \
  templates/project --name my-project
```

Use the branch containing the template. Empty dependency paths select crates.io;
use local paths until the matching ORM 0.3.0 release is published. Authentication
is enabled by default; pass `--define with_auth=false` to omit it.

After generation, follow the project's README to create an app and migrate it.
The template retains startapp and compiled migration registry markers. Generated
models are ordinary CRUD models; the project guide documents sync integration.
