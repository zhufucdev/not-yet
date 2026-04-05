use lib_common::polling::Scheduler;
use sea_orm::{DatabaseConnection, DbErr};
use smol_str::SmolStr;

use crate::{
    UserId,
    db::{
        atom, rss,
        subscription::{self, SubscriptionId},
    },
};

#[inline]
pub async fn add_feed_subscription_for(
    user_id: UserId,
    kind: subscription::Kind,
    url: impl Into<String>,
    condition: impl Into<String>,
    mock_browser: bool,
    extra_headers: Option<SmolStr>,
    db: &DatabaseConnection,
    scheduler: &Scheduler<SubscriptionId>,
) -> Result<(), DbErr> {
    let mut sub = subscription::ActiveModel::builder()
        .set_user_id(user_id)
        .set_condition(condition)
        .set_kind(kind);
    match kind {
        subscription::Kind::Rss => {
            sub = sub.set_rss(
                rss::ActiveModel::builder()
                    .set_url(url)
                    .set_browser_ua(mock_browser)
                    .set_headers(extra_headers.map(|s| s.into())),
            )
        }
        subscription::Kind::Atom => {
            sub = sub.set_atom(
                atom::ActiveModel::builder()
                    .set_url(url)
                    .set_browser_ua(mock_browser)
                    .set_headers(extra_headers.map(|s| s.into())),
            )
        }
    }

    let active_sub = sub.insert(db).await?;
    scheduler
        .add_schedule(active_sub.schedule_trigger(), active_sub.id)
        .await
        .unwrap();
    Ok(())
}
