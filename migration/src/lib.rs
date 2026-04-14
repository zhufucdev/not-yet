pub use sea_orm_migration::prelude::*;

mod m20260329_113855_create_tables;
mod m20260402_020348_add_agent_id_to_mem;
mod m20260402_084610_create_bot_db_tables;
mod m20260404_171109_add_bot_db_sub_kind;
mod m20260404_171549_create_bot_db_atom_table;
mod m20260405_071428_add_bot_db_sub_buffer_size;
mod m20260414_034251_drop_update_key_unique_constraint;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260329_113855_create_tables::Migration),
            Box::new(m20260402_020348_add_agent_id_to_mem::Migration),
            #[cfg(feature = "bot")]
            Box::new(m20260402_084610_create_bot_db_tables::Migration),
            #[cfg(feature = "bot")]
            Box::new(m20260404_171109_add_bot_db_sub_kind::Migration),
            #[cfg(feature = "bot")]
            Box::new(m20260404_171549_create_bot_db_atom_table::Migration),
            #[cfg(feature = "bot")]
            Box::new(m20260405_071428_add_bot_db_sub_buffer_size::Migration),
            Box::new(m20260414_034251_drop_update_key_unique_constraint::Migration),
        ]
    }
}
