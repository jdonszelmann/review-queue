use std::path::PathBuf;

use rust_query::{
    Database,
    migration::{Config, schema},
};

#[schema(Schema)]
#[version(0..=0)]
pub mod vN {
    pub struct User {
        #[unique]
        pub username: String,

        /// put in issues to know which ones are old
        pub sequence_number: i64,

        pub refresh_rate_seconds: i64,
    }

    /// To keep a history of closed issues
    pub struct Issue {
        #[unique]
        pub number: i64,
        pub user: User,

        pub last_seen_sequence_number: i64,
    }

    pub struct OauthState {
        #[unique]
        pub csrf: String,
        pub pkcs: String,
        pub return_url: String,
    }
}

pub use v0::*;

pub fn migrate(db_path: PathBuf) -> Database<Schema> {
    let m = Database::migrator(Config::open(db_path))
        .expect("database is older than supported versions");

    m.finish()
        .expect("database is newer than supported versions")
}
