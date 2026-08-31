//! Dogfood/proof harness: run the cloud facts client once against a real
//! endpoint with an isolated `CODEWHALE_HOME` and print the resulting status.
//!
//! ```sh
//! CODEWHALE_HOME=$(mktemp -d) CODEWHALE_CLOUD_FACTS=1 \
//!   CODEWHALE_CLOUD_FACTS_URL=http://localhost:3000/api/facts/v1/{channel} \
//!   cargo run -p codewhale-cloud-facts --example fetch_live
//! ```
//!
//! The flag stays off by default; this example only honours the env override.

use codewhale_cloud_facts::{Settings, maybe_load_persisted_cache, refresh, status};
use codewhale_config::catalog::now_unix;
use codewhale_config::cloud_facts::overlay;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let settings = Settings::default().resolve();
    println!(
        "enabled={} channel={} url={}",
        settings.enabled,
        settings.channel,
        settings.url()
    );
    println!(
        "cache={}",
        codewhale_cloud_facts::cache_path().map_or("<none>".into(), |p| p.display().to_string())
    );
    let seeded = maybe_load_persisted_cache(&settings);
    println!("seeded_from_disk={seeded:?}");
    println!("before: {}", status().label(now_unix()));
    match refresh(&settings, true).await {
        Ok(outcome) => println!("refresh: {outcome:?}"),
        Err(err) => println!("refresh error: {err}"),
    }
    let st = status();
    println!("after:  {}", st.label(now_unix()));
    println!(
        "status_json={}",
        serde_json::to_string(&st).unwrap_or_default()
    );
    if let Some(facts) = overlay::overlay() {
        println!(
            "overlay: channel={} v{} key={} sha256={} patches={} defaults={} announcements={} dropped={}",
            facts.channel,
            facts.facts_version,
            facts.key_id,
            facts.sha256,
            facts.models.len(),
            facts.provider_defaults.len(),
            facts.announcements.len(),
            facts.dropped.len()
        );
        if let Some(release) = &facts.release {
            println!(
                "release: latest={} yanked={:?}",
                release.latest, release.yanked
            );
        }
        for a in &facts.announcements {
            println!("announcement[{}] {:?}: {}", a.id, a.level, a.text);
        }
    } else {
        println!("overlay: none (bundled facts in use)");
    }
}
