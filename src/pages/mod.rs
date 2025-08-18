use serde::{Deserialize, Serialize};

use crate::model::Author;

pub mod auth;
pub mod home;
pub mod queue;

#[derive(Deserialize)]
#[serde(tag = "key")]
pub enum QueuePageWebsocketMessageRx {
    UpdatePrs,
    UsernameSuggestions { current_value: String },
    ResetUsername,
    UsernameSelect { selected_name: String },
}

#[derive(Serialize)]
#[serde(tag = "key")]
pub enum QueuePageWebsocketMessageTx {
    UpdatePage { main_contents: String },
    SetUsername { new_name: String },
    UsernameSuggestions { suggestions: Vec<Author> },
    UsernameNotValid,
}
