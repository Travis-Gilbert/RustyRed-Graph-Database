use crate::state::ThgState;

pub trait ThgStore {
    fn load(&self) -> ThgState;
    fn save(&mut self, state: &ThgState);
}

#[derive(Clone, Debug, Default)]
pub struct InMemoryThgStore {
    state: ThgState,
}

impl InMemoryThgStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl ThgStore for InMemoryThgStore {
    fn load(&self) -> ThgState {
        self.state.clone()
    }

    fn save(&mut self, state: &ThgState) {
        self.state = state.clone();
    }
}

#[cfg(feature = "redis-store")]
#[derive(Clone, Debug)]
pub struct RedisThgStore {
    client: redis::Client,
    key: String,
}

#[cfg(feature = "redis-store")]
impl RedisThgStore {
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
impl ThgStore for RedisThgStore {
    fn load(&self) -> ThgState {
        let mut connection = match self.client.get_connection() {
            Ok(connection) => connection,
            Err(_) => return ThgState::default(),
        };
        let raw: redis::RedisResult<String> =
            redis::cmd("GET").arg(&self.key).query(&mut connection);
        raw.ok()
            .and_then(|value| serde_json::from_str::<ThgState>(&value).ok())
            .unwrap_or_default()
    }

    fn save(&mut self, state: &ThgState) {
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
    use super::{InMemoryThgStore, ThgStore};
    use crate::state::RunState;

    #[test]
    fn in_memory_store_round_trips_state() {
        let mut store = InMemoryThgStore::new();
        let mut state = store.load();
        state.runs.insert(
            "run:redis-contract".to_string(),
            RunState {
                run_id: "run:redis-contract".to_string(),
                task: "persist THG".to_string(),
                actor: "agent".to_string(),
                scope: serde_json::json!({ "source": "test" }),
                status: "running".to_string(),
                steps: Vec::new(),
            },
        );

        store.save(&state);

        let loaded = store.load();
        assert_eq!(loaded.runs["run:redis-contract"].task, "persist THG");
    }
}
