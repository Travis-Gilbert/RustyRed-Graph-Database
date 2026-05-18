use crate::state::RustyredState;

pub trait RustyredStore {
    fn load(&self) -> RustyredState;
    fn save(&mut self, state: &RustyredState);
}

#[derive(Clone, Debug, Default)]
pub struct InMemoryRustyredStore {
    state: RustyredState,
}

impl InMemoryRustyredStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl RustyredStore for InMemoryRustyredStore {
    fn load(&self) -> RustyredState {
        self.state.clone()
    }

    fn save(&mut self, state: &RustyredState) {
        self.state = state.clone();
    }
}

#[cfg(feature = "redis-store")]
#[derive(Clone, Debug)]
pub struct RedisRustyredStore {
    client: redis::Client,
    key: String,
}

#[cfg(feature = "redis-store")]
impl RedisRustyredStore {
    pub fn new(redis_url: &str, key: impl Into<String>) -> redis::RedisResult<Self> {
        Ok(Self {
            client: redis::Client::open(redis_url)?,
            key: key.into(),
        })
    }

    pub fn ping(&self) -> redis::RedisResult<()> {
        let mut connection = self.client.get_connection()?;
        redis::cmd("PING").query::<String>(&mut connection)?;
        Ok(())
    }
}

#[cfg(feature = "redis-store")]
impl RustyredStore for RedisRustyredStore {
    fn load(&self) -> RustyredState {
        let mut connection = match self.client.get_connection() {
            Ok(connection) => connection,
            Err(_) => return RustyredState::default(),
        };
        let raw: redis::RedisResult<String> =
            redis::cmd("GET").arg(&self.key).query(&mut connection);
        raw.ok()
            .and_then(|value| serde_json::from_str::<RustyredState>(&value).ok())
            .unwrap_or_default()
    }

    fn save(&mut self, state: &RustyredState) {
        let mut connection = match self.client.get_connection() {
            Ok(connection) => connection,
            Err(_) => return,
        };
        if let Ok(raw) = serde_json::to_string(state) {
            let _: redis::RedisResult<()> = redis::cmd("SET")
                .arg(&self.key)
                .arg(raw)
                .query(&mut connection);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{InMemoryRustyredStore, RustyredStore};
    use crate::state::RunState;

    #[test]
    fn in_memory_store_round_trips_state() {
        let mut store = InMemoryRustyredStore::new();
        let mut state = store.load();
        state.runs.insert(
            "run:redis-contract".to_string(),
            RunState {
                run_id: "run:redis-contract".to_string(),
                task: "persist RustyRed".to_string(),
                actor: "agent".to_string(),
                scope: serde_json::json!({ "source": "test" }),
                status: "running".to_string(),
                steps: Vec::new(),
            },
        );

        store.save(&state);

        let loaded = store.load();
        assert_eq!(loaded.runs["run:redis-contract"].task, "persist RustyRed");
    }
}
