use std::path::PathBuf;

use rust_query::{
    Database,
    migration::{Config, schema},
};

#[schema(Schema)]
#[version(0..=0)]
pub mod vN {
    pub struct Repo {
        pub owner: String,
        pub name: String,
        pub bors_url: String,
    }

    pub struct RepoUser {
        pub repo: Repo,
        pub user: User,
    }

    pub struct User {
        pub username: String,
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
