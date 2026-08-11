// best-effort throughout: discord being absent is normal and never worth a word
use std::sync::Mutex;

use discord_rich_presence::activity::{Activity, Assets, Timestamps};
use discord_rich_presence::{DiscordIpc, DiscordIpcClient};

// a public application id, not a secret
const APP_ID: &str = "1407248936741081098";

// reconnects only when something is being set, never on a timer
static CLIENT: Mutex<Option<DiscordIpcClient>> = Mutex::new(None);

fn with_client(
    f: impl FnOnce(&mut DiscordIpcClient) -> Result<(), discord_rich_presence::error::Error>,
) {
    let mut guard = match CLIENT.lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    };

    if guard.is_none() {
        let mut client = DiscordIpcClient::new(APP_ID);
        // fails instantly when the socket is absent
        if client.connect().is_err() {
            return;
        }
        *guard = Some(client);
    }

    let Some(client) = guard.as_mut() else { return };
    if f(client).is_err() {
        // the socket died; drop it so the next call reconnects
        *guard = None;
    }
}

pub fn set_playing(since_unix: i64) {
    with_client(|client| {
        client.set_activity(
            Activity::new()
                .details("The Cycle: Frontier")
                .state("Singleplayer")
                .timestamps(Timestamps::new().start(since_unix))
                .assets(
                    Assets::new()
                        .large_image("fortuna")
                        .large_text("Fortuna III"),
                ),
        )
    });
}

pub fn clear() {
    with_client(|client| client.clear_activity());
}

pub fn disconnect() {
    let mut guard = match CLIENT.lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    };
    if let Some(mut client) = guard.take() {
        let _ = client.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // the whole contract: never panic or block when discord is absent
    #[test]
    fn every_call_is_safe_without_discord() {
        set_playing(0);
        clear();
        disconnect();
        // and again, to prove the failed connection was not cached as live
        set_playing(1);
        disconnect();
    }

    #[test]
    fn the_app_id_is_a_discord_snowflake() {
        assert!(APP_ID.len() >= 17 && APP_ID.chars().all(|c| c.is_ascii_digit()));
    }
}
