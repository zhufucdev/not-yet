pub use sea_orm_migration::prelude::*;

mod m20260329_113855_create_tables;
mod m20260402_020348_add_agent_id_to_mem;
mod m20260402_084610_create_bot_db_tables;
mod m20260404_171109_add_bot_db_sub_kind;
mod m20260404_171549_create_bot_db_atom_table;
mod m20260405_071428_add_bot_db_sub_buffer_size;
mod m20260501_112246_add_criteria_mem_table;
mod m20260501_134800_add_bot_db_msg_id_by_dialog_id;
mod m20260531_104434_add_bot_channel;
mod m20260602_055534_add_bot_dialog_chat_id;
mod m20260602_064052_change_bot_msg_id_by_dialog_id_pk;
mod m20260607_041240_add_bot_broadcast;

pub(crate) mod id_msg_id_by_dialog_id;
pub(crate) mod id_notify;
pub(crate) mod id_subscription;
pub(crate) mod id_user;
pub(crate) mod id_broadcast;

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
            Box::new(m20260501_112246_add_criteria_mem_table::Migration),
            #[cfg(feature = "bot")]
            Box::new(m20260501_134800_add_bot_db_msg_id_by_dialog_id::Migration),
            #[cfg(feature = "bot")]
            Box::new(m20260531_104434_add_bot_channel::Migration),
            #[cfg(feature = "bot")]
            Box::new(m20260602_055534_add_bot_dialog_chat_id::Migration),
            #[cfg(feature = "bot")]
            Box::new(m20260602_064052_change_bot_msg_id_by_dialog_id_pk::Migration),
            #[cfg(feature = "bot")]
            Box::new(m20260607_041240_add_bot_broadcast::Migration),
        ]
    }
}
