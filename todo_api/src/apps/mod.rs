pub mod tasks;

use che_rest::InstalledApps;

pub fn installed_apps() -> InstalledApps {
    InstalledApps::new().add(tasks::module())
}
