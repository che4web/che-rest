pub mod notifications;
pub mod tasks;

use che_rest::InstalledApps;

pub fn installed_apps() -> InstalledApps {
    InstalledApps::new()
        .add(che_rest::auth::module())
        .add(tasks::module())
        .add(notifications::module())
}
