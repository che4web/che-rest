// che-rest:startapp modules
{% if layout == "modular" -%}
pub mod tasks;
{% endif %}
use che_rest::InstalledApps;

pub fn installed_apps() -> InstalledApps {
{% if with_auth %}    InstalledApps::new()
        .add(che_rest::auth::module())
        .add({% if layout == "single" %}crate{% else %}tasks{% endif %}::module())
    // che-rest:startapp installed apps
{% else %}    InstalledApps::new().add({% if layout == "single" %}crate{% else %}tasks{% endif %}::module())
    // che-rest:startapp installed apps
{% endif %}}
