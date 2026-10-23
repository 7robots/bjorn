//! A live run against a Craft space: `cargo run --example craft_live`.
//! Reads the `[craft]` table of the config (or the file named first); makes one scratch document in
//! Unsorted, edits it, and moves it to the trash.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bjorn::bear::BearClient;
use bjorn::config::Config;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).map(std::path::PathBuf::from);
    let config = Config::load(path.as_deref())?;
    let store = bjorn::craft::CraftStore::connect(&config.craft).map_err(|e| anyhow::anyhow!(e))?;
    let client = Arc::new(BearClient::craft(store));
    let t = Instant::now();
    let snap = client.snapshot().await?;
    println!("snapshot: {} notes in {:?}", snap.notes.len(), t.elapsed());
    // Let the background walk place folders.
    for _ in 0..30 {
        tokio::time::sleep(Duration::from_secs(10)).await;
        let snap = client.snapshot().await?;
        let tagged = snap.notes.iter().filter(|n| !n.tags.is_empty()).count();
        let previews = snap.notes.iter().filter(|n| !n.preview.is_empty()).count();
        println!(
            "{:>4}s tagged {tagged}, previews {previews}",
            t.elapsed().as_secs()
        );
        if tagged > snap.notes.len() * 9 / 10 {
            break;
        }
    }
    client.save_preview_cache();
    let snap = client.snapshot().await?;
    let spec = snap
        .notes
        .iter()
        .find(|n| n.tags.iter().any(|t| t == "techne/dev/specs"))
        .unwrap();
    println!("a spec: {} {:?}", spec.title, spec.tags);
    let body = client.cat(&spec.id).await?;
    println!(
        "--- first 600 chars\n{}",
        body.content.chars().take(600).collect::<String>()
    );
    println!("todo rows: {}", client.todo_rows("").await?.len());
    println!(
        "search 'bjorn': {}",
        client.search_ids("bjorn", "notes").await?.len()
    );

    let id = client
        .create(
            "zz bjorn live test",
            &[],
            "# zz bjorn live test\n\nFirst para.\n\n## Tasks\n\n- [ ] alpha\n- [ ] beta\n",
        )
        .await;
    let id = match id {
        Ok(id) => id,
        Err(e) => {
            println!("create failed: {e}");
            return Ok(());
        }
    };
    let run = async {
        let read = client.cat(&id).await?;
        println!("--- created\n{}", read.content);
        let edited = read
            .content
            .replace("First para.", "First para, edited.")
            .replace("- [ ] beta", "- [ ] beta\n- [ ] gamma");
        client.overwrite(&id, &edited, &read.hash).await?;
        client
            .tick_todo(&id, "- [ ] alpha", "- [x] alpha", "## Tasks")
            .await?;
        client.append(&id, "Appended at the end.", None).await?;
        let read = client.cat(&id).await?;
        println!("--- after edits\n{}", read.content);
        let stale = client
            .overwrite(&id, "# zz bjorn live test\n\nclobber", "deadbeef")
            .await;
        println!(
            "stale write refused as conflict: {}",
            stale.is_err_and(|e| e.is_conflict())
        );
        anyhow::Ok(())
    };
    let outcome = run.await;
    client.trash(&id).await?;
    println!("trashed scratch doc");
    outcome
}
