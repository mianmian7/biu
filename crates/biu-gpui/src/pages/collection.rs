use super::*;
use crate::state::CollectionKind;

#[derive(Default)]
pub(super) struct Collection {
    input: Option<Entity<InputState>>,
    keyword: String,
    page: u64,
    results: Load<Page<TrackItem>>,
}

impl PagesHost {
    pub(super) fn enter_collection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(
            self.route,
            Some(Route::Collection {
                kind: CollectionKind::FavFolder,
                ..
            })
        ) {
            self.data.collection.input =
                Some(self.filter_input("筛选收藏夹", window, cx, Self::filter_collection));
        }
        self.load_collection(1, cx);
    }

    fn filter_collection(&mut self, keyword: String, cx: &mut Context<Self>) {
        self.data.collection.keyword = keyword;
        self.load_collection(1, cx);
    }

    fn load_collection(&mut self, page: u64, cx: &mut Context<Self>) {
        let Some(Route::Collection { kind, id, mid, .. }) = self.route else {
            return;
        };
        self.data.collection.page = page;
        let keyword = self.data.collection.keyword.clone();
        self.load(
            |h| &mut h.data.collection.results,
            move |s| collection_page(s, kind, id, mid, page, keyword),
            cx,
        );
    }

    fn all_collection(&mut self, append: bool, cx: &mut Context<Self>) {
        let Some(Route::Collection { kind, id, mid, .. }) = self.route else {
            return;
        };
        let keyword = self.data.collection.keyword.clone();
        self.queue_all(
            move |s, page| collection_page(s, kind, id, mid, page, keyword.clone()),
            append,
            cx,
        );
    }

    pub(super) fn collection_view(&self, p: BiuPalette, cx: &Context<Self>) -> Div {
        let Some(Route::Collection {
            kind, id, title, ..
        }) = self.route.as_ref()
        else {
            return div();
        };
        let data = &self.data.collection;
        let state = self.state.read(cx);
        let owned = (*kind == CollectionKind::FavFolder)
            .then(|| state.created_folders.iter().find(|folder| folder.id == *id))
            .flatten();
        let mut root = page(owned.map_or_else(|| title.clone(), |folder| folder.title.clone()));
        if let Some(folder) = owned.cloned() {
            root = root.child(toolbar().child(state_button(
                &self.state,
                "edit-folder",
                "编辑收藏夹",
                p,
                move |s, cx| {
                    s.open_modal(
                        ActiveModal::FavFolderEdit {
                            folder: Some(folder.clone()),
                        },
                        cx,
                    );
                },
            )));
        }
        if let Some(input) = &data.input {
            root = root.child(filter_bar(input, "筛选", p, cx, Self::filter_collection));
        }
        let folder_id = *id;
        root.child(data.results.content(
            p,
            button("retry", "重试", p).on_click(
                cx.listener(|this, _, _, cx| this.load_collection(this.data.collection.page, cx)),
            ),
            |result| {
                column()
                    .child(self.paginated_actions(result, data.page, p, Self::all_collection, cx))
                    .when(result.items.is_empty(), |c| {
                        c.child(empty_state("没有找到歌曲", "", p))
                    })
                    .children(result.items.iter().enumerate().map(|(i, track)| {
                        let resource = playable_id(track);
                        track_row(&self.state, i, track, p, cx).when(
                            owned.is_some() && !self.mutating,
                            |row| {
                                row.when_some(resource, |row, resource| {
                                    row.child(button("remove", "移出", p).on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            this.mutate(
                                                move |s| async move {
                                                    s.bili
                                                        .batch_del_fav_resources(
                                                            folder_id,
                                                            &[resource],
                                                        )
                                                        .await
                                                },
                                                "已移出收藏夹",
                                                Self::refresh_collection,
                                                cx,
                                            );
                                        },
                                    )))
                                })
                            },
                        )
                    }))
                    .child(pager(
                        result,
                        data.page,
                        p,
                        page_change(cx, Self::load_collection),
                    ))
            },
        ))
    }

    fn refresh_collection(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, AppState::refresh_user_and_folders);
        self.load_collection(self.data.collection.page, cx);
    }
}

async fn collection_page(
    services: Arc<Services>,
    kind: CollectionKind,
    id: u64,
    mid: u64,
    page: u64,
    keyword: String,
) -> anyhow::Result<Page<TrackItem>> {
    let keyword = (!keyword.is_empty()).then_some(keyword.as_str());
    let bili = &services.bili;
    match kind {
        CollectionKind::FavFolder => bili.get_fav_folder_resources(id, page, keyword).await,
        CollectionKind::Season => bili.get_season_archives(mid, id, page).await,
        CollectionKind::Series => bili.get_series_archives(mid, id, page).await,
    }
}
