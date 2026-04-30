//! REST client for the Proton Drive API.
//!
//! Endpoints we hit (see <https://github.com/ProtonDriveApps>):
//!
//! | Endpoint                                  | Purpose                                |
//! |-------------------------------------------|----------------------------------------|
//! | `POST   /auth/info`                       | Get SRP modulus + salt + ephemeral     |
//! | `POST   /auth`                            | Complete SRP, get session tokens       |
//! | `POST   /auth/2fa`                        | Submit TOTP                            |
//! | `POST   /auth/refresh`                    | Refresh access token                   |
//! | `GET    /core/v4/keys/salts`              | Mailbox-password salt (PGP unlock)     |
//! | `GET    /drive/shares`                    | List user's shares                     |
//! | `GET    /drive/shares/{shareID}`          | Share metadata + share key             |
//! | `GET    /drive/shares/{shareID}/folders/{linkID}/children` | List children    |
//! | `GET    /drive/shares/{shareID}/links/{linkID}` | Link metadata + node key         |
//! | `GET    /drive/shares/{shareID}/files/{linkID}/revisions/{revID}` | File rev|
//! | `GET    /drive/shares/{shareID}/blocks/{blockURL}` | Download an encrypted block   |
//! | `GET    /drive/v2/events?from={eventID}`  | Sync poller                            |
//!
//! Block content is fetched from a CDN URL embedded in the revision metadata
//! (signed). The Drive API itself is at `https://drive-api.proton.me/`.

pub mod client;
pub mod events;
pub mod model;

pub use client::ApiClient;
