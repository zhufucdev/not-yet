use lib_common::polling::Scheduler;
use sea_orm::{DatabaseConnection, DbErr};
use smol_str::SmolStr;

use crate::{
    UserId,
    db::{
        rss,
        subscription::{self, SubscriptionId},
    },
};

#[inline]
pub async fn add_rss_subscription_for(
    user_id: UserId,
    url: impl Into<String>,
    condition: impl Into<String>,
    mock_browser: bool,
    extra_headers: Option<SmolStr>,
    db: &DatabaseConnection,
    scheduler: &Scheduler<SubscriptionId>,
) -> Result<(), DbErr> {
    let sub = subscription::ActiveModel::builder()
        .set_user_id(user_id)
        .set_condition(condition)
        .set_rss(
            rss::ActiveModel::builder()
                .set_url(url)
                .set_browser_ua(mock_browser)
                .set_headers(extra_headers.map(|s| s.into())),
        )
        .save(db)
        .await?;
    let sub_id = *sub.id.as_ref();
    scheduler
        .add_schedule(sub.schedule_trigger(), sub_id)
        .await
        .unwrap();
    Ok(())
}
