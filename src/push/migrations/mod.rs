// che-orm: migrations begin
pub mod m0001_initial;
pub fn all() -> Vec<che_orm::Migration> {
    vec![m0001_initial::migration()]
}
// che-orm: migrations end
