pub use sea_orm_migration::prelude::*;

mod m20260329_113855_create_tables;
mod m20260402_020348_add_agent_id_to_mem;
mod m20260402_084610_create_bot_db_tables;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260329_113855_create_tables::Migration),
            Box::new(m20260402_020348_add_agent_id_to_mem::Migration),
            #[cfg(feature = "bot")]
            Box::new(m20260402_084610_create_bot_db_tables::Migration),
        ]
    }
}
