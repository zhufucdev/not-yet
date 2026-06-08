use std::{collections::HashMap, sync::Arc};

use axum::{
    Extension,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use tokio::sync::RwLock;

use crate::meta;

#[derive(Debug, Default)]
pub struct ServerState(RwLock<HashMap<String, Arc<Broadcast>>>);

pub struct RssServer {
    bind: String,
    host: Option<Host>,
    pub state: Arc<ServerState>,
}

#[derive(Debug)]
pub struct Broadcast {
    key: String,
    title: String,
    description: String,
    items: RwLock<Vec<rss::Item>>,
}

impl RssServer {
    pub fn new(
        bind: impl ToString,
        host: Option<impl ToString>,
        broadcasts: impl IntoIterator<Item = Broadcast>,
    ) -> Self {
        Self::from_state(bind, host, Arc::new(ServerState::new(broadcasts)))
    }

    pub fn from_state(
        bind: impl ToString,
        host: Option<impl ToString>,
        state: Arc<ServerState>,
    ) -> Self {
        Self {
            bind: bind.to_string(),
            host: host.map(|s| Host(s.to_string())),
            state,
        }
    }

    pub async fn run(&self) -> Result<(), std::io::Error> {
        let app = axum::Router::new()
            .route("/{key}.rss", get(get_rss_by_key))
            .route("/", get(get_root))
            .layer(Extension(self.host.clone()))
            .with_state(Arc::clone(&self.state));
        let listener = tokio::net::TcpListener::bind(self.bind.clone()).await?;
        axum::serve(listener, app).await
    }

    pub fn state(&self) -> Arc<ServerState> {
        Arc::clone(&self.state)
    }
}

impl ServerState {
    fn new(data: impl IntoIterator<Item = Broadcast>) -> Self {
        Self(RwLock::new(
            data.into_iter()
                .map(|b| (b.key.clone(), Arc::new(b)))
                .collect(),
        ))
    }

    pub async fn broadcast(&self, key: impl AsRef<str>) -> Option<Arc<Broadcast>> {
        let key = key.as_ref();
        self.0.read().await.get(key).cloned()
    }

    pub async fn broadcasting(
        &self,
        key: impl ToString,
        title: impl ToString,
        description: impl ToString,
    ) -> Arc<Broadcast> {
        let key = key.to_string();
        let mut guard = self.0.write().await;
        match guard.get(&key) {
            Some(existing) => Arc::clone(existing),
            None => {
                let b = Arc::new(Broadcast::new(key.clone(), title, description, []));
                guard.insert(key, Arc::clone(&b));
                b
            }
        }
    }

    pub async fn stop_broadcasting(&self, key: impl AsRef<str>) {
        self.0.write().await.remove(key.as_ref());
    }
}

impl FromIterator<Broadcast> for ServerState {
    fn from_iter<T: IntoIterator<Item = Broadcast>>(iter: T) -> Self {
        Self::new(iter)
    }
}

async fn get_rss_by_key(
    State(state): State<Arc<ServerState>>,
    Path(key): Path<String>,
    Extension(maybe_host): Extension<Option<Host>>,
) -> GetRssByKeyResponse {
    let state_guard = state.0.read().await;
    let Some(feed) = state_guard.get(&key) else {
        return GetRssByKeyResponse::KeyNotFound;
    };
    let channel = rss::ChannelBuilder::default()
        .items(feed.items.read().await.clone())
        .title(feed.title.clone())
        .description(feed.description.clone())
        .link(maybe_host.map(|h| h.0).unwrap_or_default())
        .build();
    GetRssByKeyResponse::Ok(channel)
}

async fn get_root() -> String {
    format!("not-yet {}", meta::VERSION.unwrap_or("dev"))
}

#[derive(Debug, Clone)]
struct Host(String);

impl Broadcast {
    pub fn new(
        key: impl ToString,
        title: impl ToString,
        description: impl ToString,
        items: impl IntoIterator<Item = rss::Item>,
    ) -> Self {
        Self {
            key: key.to_string(),
            title: title.to_string(),
            description: description.to_string(),
            items: RwLock::new(items.into_iter().collect()),
        }
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub async fn push_item(&self, item: rss::Item) {
        self.items.write().await.push(item);
    }
}

enum GetRssByKeyResponse {
    KeyNotFound,
    Ok(rss::Channel),
}

impl IntoResponse for GetRssByKeyResponse {
    fn into_response(self) -> Response {
        match self {
            GetRssByKeyResponse::KeyNotFound => {
                (StatusCode::NOT_FOUND, "key not found").into_response()
            }
            GetRssByKeyResponse::Ok(channel) => (
                [(header::CONTENT_TYPE, "application/rss+xml")],
                channel.to_string(),
            )
                .into_response(),
        }
    }
}
