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
