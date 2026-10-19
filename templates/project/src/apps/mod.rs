// che-rest:startapp modules

use che_rest::InstalledApps;

pub fn installed_apps() -> InstalledApps {
    InstalledApps::new()
{% if with_auth %}        .add(che_rest::auth::module())
{% endif %}        // che-rest:startapp installed apps
}
