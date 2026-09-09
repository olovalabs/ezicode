use gpui::{actions, Action};
use serde::Deserialize;

#[derive(Clone, Action, PartialEq, Eq, Deserialize)]
#[action(namespace = ui, no_json)]
pub struct Confirm {

    pub secondary: bool,
}

actions!(ui, [Cancel, SelectUp, SelectDown, SelectLeft, SelectRight]);
