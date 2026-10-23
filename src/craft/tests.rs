use std::sync::Arc;

use pretty_assertions::assert_eq;

use super::fake::{FakeCraft, Shared};
use super::*;
use crate::bear::BearClient;

struct World {
    craft: Arc<FakeCraft>,
    store: Arc<CraftStore>,
    techne: String,
    specs: String,
    plan: String,
    loose: String,
    gone: String,
}

fn world() -> World {
    world_with(CraftConfig::default())
}

fn world_with(config: CraftConfig) -> World {
    let craft = Arc::new(FakeCraft::default());
    let (techne, specs, plan, loose, gone) = {
        let mut space = craft.space.lock().unwrap();
        let techne = space.add_folder("techne", None);
        let dev = space.add_folder("dev", Some(&techne));
        let specs = space.add_folder("specs", Some(&dev));
        space.add_doc("Top", &techne, "At the top");
        let plan = space.add_doc(
            "Plan",
            &specs,
            "Intro line\n\n## Tasks\n\n- [ ] one\n- [ ] two\n\n## Notes\n\nSee [Top](block://d4)",
        );
        let loose = space.add_doc("Loose", "unsorted", "nothing filed");
        let gone = space.add_doc("Gone", "trash", "deleted");
        (techne, specs, plan, loose, gone)
    };
    let store = Arc::new(CraftStore::new(
        Arc::new(Shared(craft.clone())),
        Arc::new(Shared(craft.clone())),
        &config,
    ));
    World {
        craft,
        store,
        techne,
        specs,
        plan,
        loose,
        gone,
    }
}

impl World {
    /// One background cycle, run in the foreground.
    async fn cycle(&self) {
        let t = Shared(self.craft.clone());
        self.store.refresh(&t).await.unwrap();
        self.store.walk(&t).await.unwrap();
        self.store
            .fill_bodies(&t, Instant::now() + Duration::from_secs(60))
            .await;
    }

    async fn note(&self, id: &str) -> Note {
        let snap = self.store.snapshot().await.unwrap();
        snap.by_id(id).cloned().expect("note in snapshot")
    }
}

#[tokio::test]
async fn folders_become_tags_and_the_trash_is_the_trash() {
    let w = world();
    w.cycle().await;
    let plan = w.note(&w.plan).await;
    assert_eq!(plan.title, "Plan");
    assert_eq!(plan.tags, vec!["techne", "techne/dev", "techne/dev/specs"]);
    assert_eq!(plan.location, Location::Notes);
    assert_eq!((plan.todos, plan.done), (2, 0));
    assert!(plan.preview.starts_with("Intro line"), "{}", plan.preview);
    assert_eq!(w.note(&w.loose).await.tags, Vec::<String>::new());
    assert_eq!(w.note(&w.gone).await.location, Location::Trash);
    assert_eq!(
        w.store.tag_list(),
        vec!["techne", "techne/dev", "techne/dev/specs"]
    );
}

#[tokio::test]
async fn a_note_reads_as_markdown_with_wiki_links() {
    let w = world();
    w.cycle().await;
    let content = w.store.cat(&w.plan).await.unwrap().content;
    assert_eq!(
        content,
        "# Plan\n\nIntro line\n\n## Tasks\n\n- [ ] one\n- [ ] two\n\n## Notes\n\nSee [[Top]]\n"
    );
}

#[tokio::test]
async fn an_edit_writes_only_the_blocks_that_changed() {
    let w = world();
    w.cycle().await;
    let before = w.craft.blocks(&w.plan);
    let read = w.store.cat(&w.plan).await.unwrap();
    let edited = read
        .content
        .replace("- [ ] two", "- [x] two\n- [ ] three")
        .replace("See [[Top]]", "See [[Top]] again");
    w.craft.clear_log();
    w.store
        .overwrite(&w.plan, &edited, &read.hash)
        .await
        .unwrap();
    let writes: Vec<String> = w
        .craft
        .requests()
        .into_iter()
        .filter(|r| !r.starts_with("GET"))
        .collect();
    assert_eq!(writes, vec!["PUT /blocks", "POST /blocks"]);
    assert_eq!(
        w.craft.body_of(&w.plan),
        vec![
            "Intro line",
            "## Tasks",
            "- [ ] one",
            "- [x] two",
            "- [ ] three",
            "## Notes",
            "See [Top](block://d4) again"
        ]
    );
    // Untouched blocks kept their ids.
    let after = w.craft.blocks(&w.plan);
    for (id, md) in &before {
        if md != "- [ ] two" && !md.starts_with("See") {
            assert_eq!(after.get(id), Some(md));
        }
    }
    // The cache holds what was written, and keeps it once the next listing
    // brings the stamp the write produced.
    let cached = || {
        w.store
            .state
            .lock()
            .unwrap()
            .body(&w.plan)
            .map(str::to_string)
    };
    assert_eq!(cached(), Some(format!("{}\n", edited.trim_end())));
    w.store.refresh(&Shared(w.craft.clone())).await.unwrap();
    assert_eq!(cached(), Some(format!("{}\n", edited.trim_end())));
}

#[tokio::test]
async fn a_stale_base_is_a_conflict() {
    let w = world();
    w.cycle().await;
    let read = w.store.cat(&w.plan).await.unwrap();
    {
        let mut space = w.craft.space.lock().unwrap();
        let doc = space.docs.iter_mut().find(|d| d.id == w.plan).unwrap();
        doc.blocks[0].markdown = "Changed in Craft".into();
    }
    let err = w
        .store
        .overwrite(&w.plan, &read.content.replace("one", "uno"), &read.hash)
        .await
        .unwrap_err();
    assert!(err.is_conflict(), "{err:?}");
}

#[tokio::test]
async fn todo_ticks_and_appends_go_through_the_client() {
    let w = world();
    w.cycle().await;
    let client = BearClient::craft_store(w.store.clone());
    client
        .tick_todo(&w.plan, "- [ ] one", "- [x] one", "## Tasks")
        .await
        .unwrap();
    client
        .append(&w.plan, "- [ ] four", Some("## Tasks"))
        .await
        .unwrap();
    client.append(&w.plan, "The end", None).await.unwrap();
    assert_eq!(
        w.craft.body_of(&w.plan),
        vec![
            "Intro line",
            "## Tasks",
            "- [x] one",
            "- [ ] two",
            "- [ ] four",
            "## Notes",
            "See [Top](block://d4)",
            "The end"
        ]
    );
}

#[tokio::test]
async fn a_new_note_goes_in_its_tags_folder_made_as_needed() {
    let w = world();
    w.cycle().await;
    let (id, title, _) = w
        .store
        .create(
            None,
            &["techne/dev/new one".into()],
            "# Fresh\n\nBody text\n\n- a\n- b",
            false,
        )
        .await
        .unwrap();
    assert_eq!(title, "Fresh");
    assert_eq!(w.craft.body_of(&id), vec!["Body text", "- a", "- b"]);
    let note = w.note(&id).await;
    assert_eq!(
        note.tags,
        vec!["techne", "techne/dev", "techne/dev/new one"]
    );
    // The folder is real, and a second note reuses it.
    let (again, _, _) = w
        .store
        .create(Some("Second"), &["techne/dev/new one".into()], "", false)
        .await
        .unwrap();
    let space = w.craft.space.lock().unwrap();
    assert_eq!(
        space.folders.iter().filter(|f| f.name == "new one").count(),
        1
    );
    assert_eq!(
        space.doc(&again).unwrap().folder,
        space.doc(&id).unwrap().folder
    );
}

#[tokio::test]
async fn create_if_missing_finds_a_title_in_any_case() {
    let w = world();
    w.cycle().await;
    let (id, _, location) = w.store.create(Some("plan"), &[], "", true).await.unwrap();
    assert_eq!((id, location), (w.plan.clone(), Location::Notes));
}

#[tokio::test]
async fn trash_and_restore_return_a_note_to_its_folder() {
    let w = world();
    w.cycle().await;
    w.store.trash(&w.plan).await.unwrap();
    assert_eq!(w.note(&w.plan).await.location, Location::Trash);
    w.store.restore(&w.plan).await.unwrap();
    assert_eq!(
        w.craft.space.lock().unwrap().doc(&w.plan).unwrap().folder,
        w.specs
    );
    w.cycle().await;
    let note = w.note(&w.plan).await;
    assert_eq!((note.location, note.tags.len()), (Location::Notes, 3));
}

#[tokio::test]
async fn the_archive_is_a_folder_when_one_is_named() {
    let w = world();
    w.cycle().await;
    assert!(w.store.archive(&w.plan).await.is_err());

    let w = world_with(CraftConfig {
        archive_folder: "Archive".into(),
        ..CraftConfig::default()
    });
    w.cycle().await;
    w.store.archive(&w.plan).await.unwrap();
    assert_eq!(w.note(&w.plan).await.location, Location::Archive);
    w.cycle().await;
    assert_eq!(w.note(&w.plan).await.location, Location::Archive);
    w.store.restore(&w.plan).await.unwrap();
    assert_eq!(w.note(&w.plan).await.location, Location::Notes);
    assert_eq!(
        w.craft.space.lock().unwrap().doc(&w.plan).unwrap().folder,
        w.specs
    );
}

#[tokio::test]
async fn a_move_made_in_craft_is_found_by_the_next_walk() {
    let w = world();
    w.cycle().await;
    let move_to = |id: &str, folder: &str| {
        let mut space = w.craft.space.lock().unwrap();
        space.docs.iter_mut().find(|d| d.id == id).unwrap().folder = folder.to_string();
    };
    move_to(&w.plan, &w.techne);
    w.cycle().await;
    assert_eq!(w.note(&w.plan).await.tags, vec!["techne"]);
    move_to(&w.loose, &w.specs);
    w.cycle().await;
    assert_eq!(
        w.note(&w.loose).await.tags,
        vec!["techne", "techne/dev", "techne/dev/specs"]
    );
    move_to(&w.loose, "unsorted");
    w.cycle().await;
    assert_eq!(w.note(&w.loose).await.tags, Vec::<String>::new());
}

/// A move that leaves every count where it was is only seen by the full
/// walk, which runs every half hour.
#[tokio::test]
async fn a_swap_is_found_by_the_full_walk() {
    let w = world();
    w.cycle().await;
    {
        let mut space = w.craft.space.lock().unwrap();
        space
            .docs
            .iter_mut()
            .find(|d| d.id == w.plan)
            .unwrap()
            .folder = w.techne.clone();
        space
            .docs
            .iter_mut()
            .find(|d| d.id == w.loose)
            .unwrap()
            .folder = w.specs.clone();
    }
    w.store.state.lock().unwrap().last_full_walk = None;
    w.cycle().await;
    assert_eq!(w.note(&w.plan).await.tags, vec!["techne"]);
    assert_eq!(w.note(&w.loose).await.tags.len(), 3);
}

#[tokio::test]
async fn an_unchanged_space_costs_three_listings_a_cycle() {
    let w = world();
    w.cycle().await;
    w.craft.clear_log();
    // The first walk was full; force the next to be incremental.
    w.cycle().await;
    assert_eq!(
        w.craft.requests(),
        vec!["GET /folders", "GET /documents", "GET /documents"]
    );
}

#[tokio::test]
async fn search_speaks_enough_bear() {
    let w = world();
    w.cycle().await;
    let ids =
        |rows: Vec<Value>| -> Vec<String> { rows.iter().map(|r| text(r.get("id"))).collect() };
    let s = &w.store;
    assert_eq!(
        ids(s.search_rows("@todo", "notes", None).await.unwrap()),
        vec![w.plan.clone()]
    );
    assert_eq!(
        ids(s.search_rows("#techne/dev", "notes", None).await.unwrap()),
        vec![w.plan.clone()]
    );
    assert_eq!(
        ids(s.search_rows("filed", "notes", None).await.unwrap()),
        vec![w.loose.clone()]
    );
    assert_eq!(
        ids(s.search_rows("\"[[Top\"", "notes", None).await.unwrap()),
        vec![w.plan.clone()]
    );
    // Trashed bodies are not read ahead: the trash is searched by title.
    assert_eq!(
        ids(s.search_rows("gone", "trash", None).await.unwrap()),
        vec![w.gone.clone()]
    );
    assert!(ids(s.search_rows("gone", "notes", None).await.unwrap()).is_empty());
    assert!(
        ids(s
            .search_rows("-@untagged filed", "all", None)
            .await
            .unwrap())
        .is_empty()
    );
}

#[tokio::test]
async fn pins_are_kept_by_bjorn() {
    let dir = tempfile::tempdir().unwrap();
    let w = world();
    let store = Arc::new(
        CraftStore::new(
            Arc::new(Shared(w.craft.clone())),
            Arc::new(Shared(w.craft.clone())),
            &CraftConfig::default(),
        )
        .with_files(dir.path().join("cache.json"), dir.path().join("pins.json")),
    );
    store.pin(&w.plan, "global", true).unwrap();
    let snap = store.snapshot().await.unwrap();
    assert!(snap.by_id(&w.plan).unwrap().pinned_globally());
    // A new store reads the pins and the cache back.
    store.save();
    let again = CraftStore::new(
        Arc::new(Shared(w.craft.clone())),
        Arc::new(Shared(w.craft.clone())),
        &CraftConfig::default(),
    )
    .with_files(dir.path().join("cache.json"), dir.path().join("pins.json"));
    assert_eq!(
        again.pins.lock().unwrap().get(&w.plan).unwrap(),
        &vec!["global".to_string()]
    );
    assert!(again.state.lock().unwrap().docs.contains_key(&w.plan));
}

#[tokio::test]
async fn the_first_index_reports_progress_until_it_is_done() {
    let w = world();
    w.store.refresh(&Shared(w.craft.clone())).await.unwrap();
    let (done, total) = w
        .store
        .snapshot()
        .await
        .unwrap()
        .indexing
        .expect("indexing");
    assert!(done < total, "{done} of {total}");
    w.cycle().await;
    assert_eq!(w.store.snapshot().await.unwrap().indexing, None);
    // Once done, a note new in Craft does not bring the gauge back.
    w.craft
        .space
        .lock()
        .unwrap()
        .add_doc("Fresh", "unsorted", "new");
    w.store.refresh(&Shared(w.craft.clone())).await.unwrap();
    assert_eq!(w.store.snapshot().await.unwrap().indexing, None);
}
