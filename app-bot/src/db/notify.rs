use sea_orm::prelude::*;
#[cfg(feature = "telegram")]
use teloxide::types::{ChatId, Recipient};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "notify")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    #[sea_orm(default_value = "0")]
    pub kind: Kind,
    pub channel_id: Option<String>,
    pub chat_id: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum, DeriveDisplay)]
#[sea_orm(rs_type = "i32", db_type = "Integer")]
pub enum Kind {
    #[strum(to_string = "Chat")]
    Chat = 0,
    #[strum(to_string = "Channel")]
    Channel = 1,
}

impl ActiveModelBehavior for ActiveModel {}

#[cfg(feature = "telegram")]
impl Into<Recipient> for &Model {
    fn into(self) -> Recipient {
        match self.kind {
            Kind::Chat => Recipient::Id(ChatId(
                self.chat_id.expect("expected present user_id, got none"),
            )),
            Kind::Channel => Recipient::ChannelUsername(
                self.channel_id
                    .as_ref()
                    .map(|id| format!("@{id}"))
                    .expect("expected present channel_id, got none"),
            ),
        }
    }
}

#[cfg(feature = "telegram")]
impl Into<Recipient> for Model {
    fn into(self) -> Recipient {
        (&self).into()
    }
}
