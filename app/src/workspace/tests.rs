//! Lifecycle tests use GPUI's scheduler, not a display/GPU. Native filesystem
//! and storage workers are allowed to wake the test executor from OS threads.

use super::*;
use crate::storage::{GlobalState, LayoutState, OpenTabState, StateStore, WorkspaceState};
use crate::test_support::TempDir;
use gpui::{div, IntoElement, Render, TestAppContext, VisualTestContext};

struct TestRoot(Entity<Workspace>);

impl Render for TestRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // Keep the workspace alive, without rendering terminals or restoring
        // tabs implicitly. Tests drive restoration at explicit boundaries.
        let _workspace = &self.0;
        div()
    }
}

fn workspace_with_store<'a>(
    cx: &'a mut TestAppContext,
    store: StateStore,
    ready: async_channel::Receiver<GlobalState>,
) -> (Entity<Workspace>, &'a mut VisualTestContext) {
    cx.executor().allow_parking();
    cx.update(gpui_component::init);
    let (root, cx) = cx.add_window_view(|window, cx| {
        TestRoot(cx.new(|cx| Workspace::new_with_storage(window, cx, store, ready)))
    });
    (root.read_with(cx, |root, _| root.0.clone()), cx)
}

fn workspace<'a>(
    cx: &'a mut TestAppContext,
    fixture: &TempDir,
) -> (Entity<Workspace>, &'a mut VisualTestContext) {
    let (store, ready) = StateStore::for_directory(fixture.directory("config"));
    workspace_with_store(cx, store, ready)
}

fn buffer(text: &str) -> Result<LoadedBuffer, String> {
    Ok(LoadedBuffer {
        text: text.into(),
        lang_id: "text",
        highlight: true,
    })
}

#[gpui::test]
async fn first_folder_open_is_asynchronous_and_same_folder_is_a_no_op(cx: &mut TestAppContext) {
    let fixture = TempDir::new("first-folder");
    let root = fixture.directory("project");
    fixture.file("project/src/file.txt", "text");
    fixture.file("project/node_modules/dependency/index.js", "ignored");
    let (workspace, cx) = workspace(cx, &fixture);
    cx.update(|_, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.load_root(root.clone(), cx);
            assert!(workspace.root.is_none());
            assert_eq!(workspace.loading_root.as_ref(), Some(&root));
        });
    });
    cx.condition(&workspace, |workspace, _| {
        workspace.root.as_ref() == Some(&root)
    })
    .await;
    cx.update(|_, cx| {
        workspace.update(cx, |workspace, cx| {
            let session = workspace.session.clone();
            let git_generation = workspace.git_watch_generation;
            assert_eq!(workspace.explorer_rows.len(), 1);
            workspace.load_root(root.clone(), cx);
            assert!(!session.is_cancelled());
            assert_eq!(workspace.git_watch_generation, git_generation);
            assert!(workspace.loading_root.is_none());
        });
    });
}

#[gpui::test]
async fn rapid_folder_requests_publish_only_the_latest_target(cx: &mut TestAppContext) {
    let fixture = TempDir::new("rapid-folders");
    let a = fixture.directory("a");
    let b = fixture.directory("b");
    let c = fixture.directory("c");
    fixture.file("c/current.txt", "text");
    let (workspace, cx) = workspace(cx, &fixture);
    cx.update(|_, cx| {
        workspace.update(cx, |workspace, cx| {
            for index in 0..30 {
                workspace.load_root(if index % 2 == 0 { a.clone() } else { b.clone() }, cx);
            }
            workspace.load_root(c.clone(), cx);
            assert!(workspace.root.is_none());
        });
    });
    cx.condition(&workspace, |workspace, _| {
        workspace.root.as_ref() == Some(&c)
    })
    .await;
    cx.run_until_parked();
    workspace.read_with(cx, |workspace, _| {
        assert_eq!(workspace.root.as_ref(), Some(&c));
        assert_eq!(workspace.explorer_rows[0].path, c.join("current.txt"));
        assert!(workspace.file_loads.is_empty());
        assert!(workspace.directory_loads.is_empty());
    });
}

#[gpui::test]
async fn repeated_switches_cancel_outgoing_sessions_and_keep_scratch_buffers(
    cx: &mut TestAppContext,
) {
    let fixture = TempDir::new("repeated-folders");
    let roots = [
        fixture.directory("a"),
        fixture.directory("b"),
        fixture.directory("c"),
    ];
    for name in ["a", "b", "c"] {
        fixture.file(&format!("{name}/file.txt"), "text");
    }
    let (workspace, cx) = workspace(cx, &fixture);
    let editor = cx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.new_file(window, cx);
            let editor = workspace.tabs[0].editor.as_ref().unwrap().clone();
            editor.update(cx, |state, cx| state.set_value("scratch text", window, cx));
            workspace.tabs[0].dirty = true;
            editor.downgrade()
        })
    });
    for root in roots.iter().cycle().take(18) {
        let old = workspace.read_with(cx, |workspace, _| workspace.session.clone());
        cx.update(|_, cx| {
            workspace.update(cx, |workspace, cx| workspace.load_root(root.clone(), cx))
        });
        cx.condition(&workspace, |workspace, _| {
            workspace.root.as_ref() == Some(root)
        })
        .await;
        assert!(old.is_cancelled());
        workspace.read_with(cx, |workspace, cx| {
            assert_eq!(workspace.tabs.len(), 1);
            assert!(workspace.tabs[0].untitled);
            assert!(workspace.tabs[0].dirty);
            assert_eq!(
                workspace.tabs[0]
                    .editor
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .value()
                    .as_str(),
                "scratch text"
            );
            assert!(editor.upgrade().is_some());
            assert!(workspace.workspace_files_cache.is_none());
        });
    }
}

#[gpui::test]
async fn invalid_target_and_failed_saves_leave_the_current_workspace_intact(
    cx: &mut TestAppContext,
) {
    let fixture = TempDir::new("failed-switch");
    let a = fixture.directory("a");
    let b = fixture.directory("b");
    let path = fixture.file("a/sub/file.txt", "original");
    let (workspace, cx) = workspace(cx, &fixture);
    cx.update(|_, cx| workspace.update(cx, |workspace, cx| workspace.load_root(a.clone(), cx)));
    cx.condition(&workspace, |workspace, _| {
        workspace.root.as_ref() == Some(&a)
    })
    .await;
    cx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.finish_open_file(
                path.clone(),
                buffer("unsaved changes"),
                true,
                true,
                window,
                cx,
            );
            workspace.tabs[0].dirty = true;
            workspace.load_root(fixture.path().join("missing"), cx);
        });
    });
    cx.condition(&workspace, |workspace, _| workspace.loading_root.is_none())
        .await;
    workspace.read_with(cx, |workspace, _| {
        assert_eq!(workspace.root.as_ref(), Some(&a));
        assert!(workspace.tabs[0].dirty);
    });
    std::fs::remove_dir_all(a.join("sub")).unwrap();
    cx.update(|_, cx| workspace.update(cx, |workspace, cx| workspace.load_root(b.clone(), cx)));
    cx.condition(&workspace, |workspace, _| workspace.loading_root.is_none())
        .await;
    workspace.read_with(cx, |workspace, cx| {
        assert_eq!(workspace.root.as_ref(), Some(&a));
        assert!(workspace.status.contains("could not save"));
        assert!(workspace.tabs[0].dirty);
        assert_eq!(
            workspace.tabs[0]
                .editor
                .as_ref()
                .unwrap()
                .read(cx)
                .value()
                .as_str(),
            "unsaved changes"
        );
    });
}

#[gpui::test]
async fn restored_tabs_are_lazy_and_closing_one_cancels_its_read(cx: &mut TestAppContext) {
    let fixture = TempDir::new("lazy-tabs");
    let root = fixture.directory("project");
    let a = fixture.file("project/a.txt", "a\nb\nc\nd\ne\nf\ng\nh");
    let b = fixture.file("project/b.txt", "second");
    let (store, ready) = StateStore::for_directory(fixture.directory("config"));
    store.save(WorkspaceState {
        root: root.clone(),
        tabs: [a.clone(), b.clone()]
            .into_iter()
            .map(|path| OpenTabState {
                path,
                preview: false,
                language_override: Some("text".into()),
                cursor: Some(crate::storage::CursorPosition {
                    line: 5,
                    character: 1,
                }),
            })
            .collect(),
        active_tab: 0,
        layout: LayoutState::default(),
        expanded_folders: Vec::new(),
        explorer_selected: None,
    });
    let (workspace, cx) = workspace_with_store(cx, store, ready);
    cx.update(|_, cx| workspace.update(cx, |workspace, cx| workspace.load_root(root.clone(), cx)));
    cx.condition(&workspace, |workspace, _| {
        workspace.root.as_ref() == Some(&root)
    })
    .await;
    cx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            assert_eq!(workspace.tabs.len(), 2);
            assert!(workspace.tabs.iter().all(|tab| tab.editor.is_none()));
            workspace.restore_active_tab(window, cx);
        });
    });
    cx.condition(&workspace, |workspace, _| {
        workspace.tabs[0].editor.is_some()
    })
    .await;
    cx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            assert!(workspace.tabs[1].editor.is_none());
            assert_eq!(
                workspace.tabs[0]
                    .editor
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .cursor_position()
                    .line,
                5
            );
            workspace.active_tab = 1;
            workspace.restore_active_tab(window, cx);
            let token = workspace.file_loads.get(&b).unwrap().token.clone();
            workspace.close_tab_at_index(1, cx);
            assert!(token.is_cancelled());
            assert!(!workspace.session.is_cancelled());
        });
    });
    cx.run_until_parked();
    workspace.read_with(cx, |workspace, _| {
        assert_eq!(workspace.tabs.len(), 1);
        assert_eq!(workspace.tabs[0].path.as_ref(), Some(&a));
    });
}

#[gpui::test]
async fn returning_to_a_folder_rejects_a_delayed_result_from_its_old_session(
    cx: &mut TestAppContext,
) {
    let fixture = TempDir::new("stale-read");
    let a = fixture.directory("a");
    let b = fixture.directory("b");
    let path = fixture.file("a/file.txt", "fresh content");
    let (workspace, cx) = workspace(cx, &fixture);
    cx.update(|_, cx| workspace.update(cx, |workspace, cx| workspace.load_root(a.clone(), cx)));
    cx.condition(&workspace, |workspace, _| {
        workspace.root.as_ref() == Some(&a)
    })
    .await;
    let old = workspace.read_with(cx, |workspace, _| workspace.session.child());
    for target in [&b, &a] {
        cx.update(|_, cx| {
            workspace.update(cx, |workspace, cx| workspace.load_root(target.clone(), cx))
        });
        cx.condition(&workspace, |workspace, _| {
            workspace.root.as_ref() == Some(target)
        })
        .await;
    }
    assert!(old.is_cancelled());
    cx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.open_file(path.clone(), window, cx);
            // Deliver at the real native-read completion boundary. A path-only
            // guard would accept this and remove the new session's load.
            workspace.complete_file_load(path.clone(), &old, buffer("stale content"), window, cx);
            assert!(workspace.tabs.is_empty());
            assert!(workspace.file_loads.contains_key(&path));
        });
    });
    cx.condition(&workspace, |workspace, _| {
        workspace.tabs.iter().any(|tab| tab.editor.is_some())
    })
    .await;
    workspace.read_with(cx, |workspace, cx| {
        assert_eq!(
            workspace.active_editor().unwrap().read(cx).value().as_str(),
            "fresh content"
        );
    });
}

#[gpui::test]
fn closing_a_named_editor_releases_its_detached_subscription(cx: &mut TestAppContext) {
    let fixture = TempDir::new("editor-release");
    let path = fixture.file("file.txt", "text");
    let (workspace, cx) = workspace(cx, &fixture);
    let weak = cx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.finish_open_file(path, buffer("text"), true, true, window, cx);
            let weak = workspace.active_editor().unwrap().downgrade();
            workspace.close_tab_at_index(0, cx);
            weak
        })
    });
    cx.run_until_parked();
    assert!(
        weak.upgrade().is_none(),
        "an event-listener closure retained the editor"
    );
}

#[gpui::test]
async fn startup_restore_never_overrides_explicit_user_intent(cx: &mut TestAppContext) {
    let fixture = TempDir::new("startup-intent");
    let previous = fixture.directory("previous");
    let (store, _ready) = StateStore::for_directory(fixture.directory("config"));
    let (ready, rx) = async_channel::bounded(1);
    let (workspace, cx) = workspace_with_store(cx, store, rx);
    cx.update(|_, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.load_root(fixture.path().join("missing"), cx)
        })
    });
    cx.condition(&workspace, |workspace, _| workspace.loading_root.is_none())
        .await;
    ready
        .try_send(GlobalState {
            last_workspace_root: Some(previous),
            ..GlobalState::default()
        })
        .unwrap();
    cx.run_until_parked();
    workspace.read_with(cx, |workspace, _| {
        assert!(workspace.root.is_none());
        assert!(workspace.loading_root.is_none());
    });
}

#[gpui::test]
async fn ordered_saves_do_not_clear_edits_made_after_the_snapshot(cx: &mut TestAppContext) {
    let fixture = TempDir::new("ordered-ui-saves");
    let path = fixture.file("file.txt", "original");
    let (workspace, cx) = workspace(cx, &fixture);
    let flushed = cx.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.finish_open_file(path.clone(), buffer("snapshot"), true, true, window, cx);
            workspace.tabs[0].dirty = true;
            workspace.write_file_async(path.clone(), "snapshot".into(), "Saved", cx);
            workspace
                .active_editor()
                .unwrap()
                .clone()
                .update(cx, |state, cx| state.set_value("newer edit", window, cx));
            workspace.storage.flush()
        })
    });
    flushed.recv().await.unwrap();
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "snapshot");
    workspace.read_with(cx, |workspace, _| assert!(workspace.tabs[0].dirty));
}
