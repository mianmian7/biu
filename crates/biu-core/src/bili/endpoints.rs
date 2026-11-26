use super::*;

impl BiliClient {
    pub async fn get_nav(&self) -> Result<UserInfo> {
        self.refresh_session_cookies().await;
        let data = self.nav_data().await?;
        if let Ok((img, sub)) = extract_wbi_keys(&data) {
            *self.wbi.lock().await = Some((Instant::now(), img, sub));
        }
        decode(data)
    }

    pub async fn generate_qrcode(&self) -> Result<QrCodeSession> {
        decode(
            self.get(
                PASSPORT,
                "/x/passport-login/web/qrcode/generate",
                Value::Null,
                false,
            )
            .await?,
        )
    }

    pub async fn poll_qrcode(&self, qrcode_key: &str) -> Result<QrPollStatus> {
        let data = self
            .get(
                PASSPORT,
                "/x/passport-login/web/qrcode/poll",
                json!({ "qrcode_key": qrcode_key }),
                false,
            )
            .await?;
        match data["code"].as_i64() {
            Some(86101) => Ok(QrPollStatus::WaitingForScan),
            Some(86090) => Ok(QrPollStatus::WaitingForConfirmation),
            Some(86038) => Ok(QrPollStatus::Expired),
            Some(0) => {
                *self.wbi.lock().await = None;
                self.save_cookies()?;
                self.refresh_session_cookies().await;
                Ok(QrPollStatus::LoggedIn {
                    refresh_token: field(&data, &["refresh_token"]).to_owned(),
                })
            }
            code => bail!(
                "unexpected QR login status {code:?}: {}",
                field(&data, &["message"])
            ),
        }
    }

    pub async fn logout(&self) -> Result<()> {
        if self.get_cookie("bili_jct").is_some()
            && let Err(err) = self
                .post(
                    PASSPORT,
                    "/login/exit/v2",
                    json!({ "gourl": BILI_REFERER }),
                    None,
                )
                .await
        {
            log::warn!("Bilibili network logout failed: {err}");
        }
        self.cookies.lock().unwrap().clear();
        *self.wbi.lock().await = None;
        *self.auth_cookie_refresh.lock().await = None;
        self.save_cookies()
    }

    pub async fn get_music_banners(&self) -> Result<Vec<BannerItem>> {
        let data = self
            .get(
                API,
                "/x/centralization/interface/new/music/banner",
                json!({ "web_location": "333.1351", "csrf": self.get_cookie("bili_jct") }),
                false,
            )
            .await?;
        Ok(list(&data["list"])
            .iter()
            .map(|row| BannerItem {
                title: strip_html_tags(field(row, &["archive_title", "title"])),
                cover: cover_url(row),
                jump_url: field(row, &["jump_url"]).to_owned(),
                author: strip_html_tags(field(row, &["author"])),
                bvid: field(row, &["bvid"]).to_owned(),
                aid: number(&row["aid"]).unwrap_or_default(),
                cid: number(&row["cid"]).unwrap_or_default(),
                extra: row
                    .as_object()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|(key, _)| {
                        !matches!(
                            key.as_str(),
                            "archive_title"
                                | "title"
                                | "cover"
                                | "jump_url"
                                | "author"
                                | "bvid"
                                | "aid"
                                | "cid"
                        )
                    })
                    .collect(),
            })
            .collect())
    }

    pub async fn get_new_music(&self) -> Result<Vec<RankItem>> {
        let data = self.get(API, "/x/centralization/interface/new/music",
            json!({ "plat": 2, "web_location": "333.1351", "csrf": self.get_cookie("bili_jct") }), false).await?;
        Ok(list(&data["list"]).iter().map(rank_item).collect())
    }

    pub async fn get_music_rank(&self) -> Result<Vec<RankItem>> {
        self.get_music_rank_page(1).await
    }

    pub async fn get_music_rank_page(&self, pn: u64) -> Result<Vec<RankItem>> {
        let data = self
            .get(
                API,
                "/x/centralization/interface/music/comprehensive/web/rank",
                json!({ "pn": pn, "ps": 20, "web_location": "333.1351" }),
                false,
            )
            .await?;
        Ok(list(&data["list"]).iter().map(rank_item).collect())
    }

    pub async fn get_region_feed_rcmd(&self, display_id: u64) -> Result<Vec<TrackItem>> {
        self.get_region_feed_rcmd_for(1003, display_id).await
    }

    pub async fn get_region_feed_rcmd_for(
        &self,
        from_region: u64,
        display_id: u64,
    ) -> Result<Vec<TrackItem>> {
        let data = self.get(API, "/x/web-interface/region/feed/rcmd", json!({
            "display_id": display_id, "request_cnt": 15, "from_region": from_region, "device": "web",
            "plat": 30, "web_location": "333.40138"
        }), true).await?;
        Ok(tracks(&data["archives"]))
    }

    pub async fn search_videos(
        &self,
        keyword: &str,
        order: &str,
        page: u64,
    ) -> Result<SearchResult> {
        self.search_videos_with_page_size(keyword, order, page, 20)
            .await
    }

    pub async fn search_videos_with_page_size(
        &self,
        keyword: &str,
        order: &str,
        page: u64,
        page_size: u64,
    ) -> Result<SearchResult> {
        let data = self
            .get(
                API,
                "/x/web-interface/wbi/search/type",
                json!({ "keyword": keyword, "order": order, "page": page, "page_size": page_size, "search_type": "video" }),
                true,
            )
            .await?;
        check_search_data(&data)?;
        Ok(search_page(&data, tracks(&data["result"])))
    }

    pub async fn search_users(
        &self,
        keyword: &str,
        page: u64,
    ) -> Result<SearchResult<FollowingUser>> {
        let data = self
            .get(
                API,
                "/x/web-interface/wbi/search/type",
                json!({ "keyword": keyword, "page": page, "search_type": "bili_user" }),
                true,
            )
            .await?;
        check_search_data(&data)?;
        Ok(search_page(
            &data,
            decode(Value::Array(list(&data["result"]).to_vec()))?,
        ))
    }

    pub async fn search_suggest(&self, term: &str) -> Result<Vec<String>> {
        self.refresh_session_cookies().await;
        let request = self
            .client()
            .get("https://s.search.bilibili.com/main/suggest")
            .query(&parameters(&json!({
                "term": term, "main_ver": "v1", "highlight": "", "func": "suggest",
                "suggest_type": "accurate", "sub_type": "tag", "tag_num": 10, "spmid": "333.1007",
                "buvid": self.get_cookie("buvid3"), "userid": self.get_cookie("DedeUserID")
            })));
        let reply = checked_body(request).await?;
        Ok(list(&reply["result"]["tag"])
            .iter()
            .map(|tag| strip_html_tags(field(tag, &["value", "name"])))
            .collect())
    }

    pub async fn get_video_view(&self, bvid: &str) -> Result<VideoInfo> {
        let data = self
            .get(API, "/x/web-interface/view", json!({ "bvid": bvid }), false)
            .await?;
        let track = video_track(&data).context("video view has no BVID")?;
        Ok(VideoInfo {
            track,
            pages: decode(Value::Array(list(&data["pages"]).to_vec()))?,
            description: field(&data, &["desc"]).to_owned(),
            raw: sanitize_fields(data),
        })
    }

    pub async fn get_video_pagelist(&self, bvid: &str) -> Result<Vec<MediaPage>> {
        decode(
            self.get(API, "/x/player/pagelist", json!({ "bvid": bvid }), false)
                .await?,
        )
    }

    pub async fn get_playurl(
        &self,
        bvid: &str,
        cid: u64,
        quality: AudioQuality,
    ) -> Result<PlayUrlInfo> {
        let cid = if cid == 0 {
            self.get_video_pagelist(bvid)
                .await?
                .first()
                .context("video has no playable pages")?
                .cid
        } else {
            cid
        };
        let data = self
            .get(
                API,
                "/x/player/wbi/playurl",
                json!({ "bvid": bvid, "cid": cid, "fnval": 4048 }),
                true,
            )
            .await?;
        self.playurl_info(&data, quality)
    }

    fn playurl_info(&self, data: &Value, quality: AudioQuality) -> Result<PlayUrlInfo> {
        let dash = &data["dash"];
        let mut regular = self.audio_streams(list(&dash["audio"]));
        let mut premium = self.audio_streams(std::slice::from_ref(&dash["flac"]["audio"]));
        premium.extend(self.audio_streams(list(&dash["dolby"]["audio"])));
        const QUALITY_ORDER: [u64; 8] = [30257, 30216, 30259, 30260, 30232, 30280, 30250, 30251];
        regular.sort_by_key(|stream| {
            std::cmp::Reverse((
                stream.bandwidth,
                QUALITY_ORDER.iter().position(|id| *id == stream.id),
            ))
        });
        let index = match quality {
            AudioQuality::Auto | AudioQuality::Lossless => 0,
            AudioQuality::High => 0,
            AudioQuality::Medium => regular.len().saturating_sub(1) / 2,
            AudioQuality::Low => regular.len().saturating_sub(1),
        };
        if index > 0 {
            let selected = regular.remove(index);
            regular.insert(0, selected);
        }
        let mut streams = if matches!(quality, AudioQuality::Auto | AudioQuality::Lossless)
            || regular.is_empty()
        {
            premium.into_iter().chain(regular).collect::<Vec<_>>()
        } else {
            regular.into_iter().chain(premium).collect()
        };
        if quality == AudioQuality::Auto {
            // Symphonia can decode FLAC and AAC directly. Keep Dolby E-AC-3 as a fallback.
            streams.sort_by_key(|stream| stream.id == 30250);
        }
        let urls = streams
            .first()
            .context("Bilibili returned no DASH audio")?
            .urls
            .clone();
        Ok(PlayUrlInfo {
            urls,
            streams,
            duration_secs: dash["duration"]
                .as_f64()
                .or_else(|| data["timelength"].as_f64().map(|ms| ms / 1000.0)),
        })
    }

    fn audio_streams(&self, items: &[Value]) -> Vec<AudioStream> {
        items
            .iter()
            .filter_map(|item| {
                let urls = self.registered_urls(
                    [&item["baseUrl"], &item["base_url"]]
                        .into_iter()
                        .chain(list(&item["backupUrl"]))
                        .chain(list(&item["backup_url"])),
                );
                (!urls.is_empty()).then(|| AudioStream {
                    id: number(&item["id"]).unwrap_or_default(),
                    codecs: field(item, &["codecs"]).to_ascii_lowercase(),
                    bandwidth: number(&item["bandwidth"]).unwrap_or_default(),
                    mime_type: field(item, &["mimeType", "mime_type"]).to_owned(),
                    urls,
                })
            })
            .collect()
    }

    pub async fn get_audio_song_info(&self, sid: u64) -> Result<TrackItem> {
        let data = self
            .get(
                BILI_ORIGIN,
                "/audio/music-service-c/web/song/info",
                json!({ "sid": sid }),
                false,
            )
            .await?;
        ensure!(data.is_object(), "Bilibili audio song not found");
        Ok(audio_track(&data, sid))
    }

    pub async fn get_audio_playurl(&self, sid: u64) -> Result<PlayUrlInfo> {
        let nav = self.get_nav().await?;
        let quality = if nav.extra.get("vipStatus").and_then(number) == Some(1) {
            3
        } else {
            2
        };
        let data = self.get(API, "/audio/music-service-c/url", json!({
            "songid": sid, "quality": quality, "privilege": 2, "mid": nav.mid, "platform": "web"
        }), false).await?;
        let urls = self.registered_urls(list(&data["cdns"]));
        ensure!(!urls.is_empty(), "Bilibili returned no audio CDN URLs");
        let id = number(&data["type"]).unwrap_or_default();
        let stream = AudioStream {
            id,
            codecs: if id == 3 { "flac" } else { "" }.into(),
            bandwidth: 0,
            mime_type: String::new(),
            urls: urls.clone(),
        };
        Ok(PlayUrlInfo {
            urls,
            streams: vec![stream],
            duration_secs: None,
        })
    }

    pub async fn report_heartbeat(&self, aid: u64, cid: u64, played_time: i64) -> Result<()> {
        let start = unix_time().saturating_sub(played_time.max(0) as u64);
        let mid = self
            .get_cookie("DedeUserID")
            .and_then(|id| id.parse::<u64>().ok());
        self.post(API, "/x/click-interface/web/heartbeat", json!({
            "aid": aid, "cid": cid, "played_time": played_time, "realtime": played_time,
            "real_played_time": played_time, "start_ts": start, "dt": 2, "outer": 0, "play_type": 0
        }), Some(json!({
            "w_aid": aid, "w_mid": mid, "w_start_ts": start, "w_dt": 2, "w_realtime": played_time,
            "w_playedtime": played_time, "w_real_played_time": played_time, "web_location": 1315873
        }))).await?;
        Ok(())
    }

    pub async fn get_created_fav_folders(&self, mid: u64) -> Result<Vec<FavFolder>> {
        let data = self
            .get(
                API,
                "/x/v3/fav/folder/created/list-all",
                json!({ "up_mid": mid }),
                false,
            )
            .await?;
        decode(Value::Array(list(&data["list"]).to_vec()))
    }

    pub async fn get_created_fav_folders_for_resource(
        &self,
        mid: u64,
        rid: u64,
    ) -> Result<Vec<(FavFolder, bool)>> {
        let data = self
            .get(
                API,
                "/x/v3/fav/folder/created/list-all",
                json!({ "up_mid": mid, "rid": rid, "type": 2 }),
                false,
            )
            .await?;
        let folders: Vec<FavFolder> = decode(Value::Array(list(&data["list"]).to_vec()))?;
        Ok(folders
            .into_iter()
            .map(|folder| {
                let contains_resource = folder.extra.get("fav_state").and_then(number) == Some(1);
                (folder, contains_resource)
            })
            .collect())
    }

    pub async fn get_collected_fav_folders(&self, mid: u64, pn: u64) -> Result<Page<FavFolder>> {
        let data = self
            .get(
                API,
                "/x/v3/fav/folder/collected/list",
                json!({ "up_mid": mid, "pn": pn, "ps": 20, "platform": "web" }),
                false,
            )
            .await?;
        Ok(Page {
            items: decode(Value::Array(list(&data["list"]).to_vec()))?,
            total: number(&data["count"]),
            has_more: data["has_more"].as_bool(),
            next_offset: None,
        })
    }

    pub async fn get_fav_folder_resources(
        &self,
        media_id: u64,
        pn: u64,
        keyword: Option<&str>,
    ) -> Result<Page<TrackItem>> {
        let data = self
            .get(
                API,
                "/x/v3/fav/resource/list",
                json!({
                    "media_id": media_id, "pn": pn, "ps": 20, "platform": "web",
                    "keyword": keyword, "tid": 0, "type": 0, "order": "mtime"
                }),
                false,
            )
            .await?;
        Ok(Page {
            items: tracks(&data["medias"]),
            total: number(&data["info"]["media_count"]),
            has_more: data["has_more"].as_bool(),
            next_offset: None,
        })
    }

    pub async fn create_fav_folder(
        &self,
        title: &str,
        intro: &str,
        privacy: bool,
    ) -> Result<FavFolder> {
        decode(
            self.post(
                API,
                "/x/v3/fav/folder/add",
                json!({ "title": title, "intro": intro, "privacy": u8::from(privacy) }),
                None,
            )
            .await?,
        )
    }

    pub async fn edit_fav_folder(
        &self,
        media_id: u64,
        title: &str,
        intro: &str,
        privacy: bool,
        cover: Option<&str>,
    ) -> Result<FavFolder> {
        decode(self.post(API, "/x/v3/fav/folder/edit",
            json!({ "media_id": media_id, "title": title, "intro": intro, "privacy": u8::from(privacy), "cover": cover }), None).await?)
    }

    pub async fn delete_fav_folder(&self, media_id: u64) -> Result<()> {
        self.post(
            API,
            "/x/v3/fav/folder/del",
            json!({ "media_ids": media_id }),
            None,
        )
        .await?;
        Ok(())
    }

    pub async fn deal_fav_resource(
        &self,
        rid: u64,
        add_ids: &[u64],
        del_ids: &[u64],
    ) -> Result<()> {
        self.deal_fav_resource_with_type(rid, 2, add_ids, del_ids)
            .await
    }

    pub async fn deal_fav_resource_with_type(
        &self,
        rid: u64,
        resource_type: u32,
        add_ids: &[u64],
        del_ids: &[u64],
    ) -> Result<()> {
        let csv = |ids: &[u64]| ids.iter().map(u64::to_string).collect::<Vec<_>>().join(",");
        self.post(
            API,
            "/x/v3/fav/resource/deal",
            json!({
                "rid": rid, "add_media_ids": csv(add_ids), "del_media_ids": csv(del_ids), "type": resource_type,
                "platform": "web", "ga": 1, "gaia_source": "web_normal"
            }),
            None,
        )
        .await?;
        Ok(())
    }

    pub async fn batch_del_fav_resources(
        &self,
        media_id: u64,
        resources: &[(u64, u32)],
    ) -> Result<()> {
        let resources = resources
            .iter()
            .map(|(id, kind)| format!("{id}:{kind}"))
            .collect::<Vec<_>>()
            .join(",");
        self.post(
            API,
            "/x/v3/fav/resource/batch-del",
            json!({ "media_id": media_id, "resources": resources, "platform": "web" }),
            None,
        )
        .await?;
        Ok(())
    }

    pub async fn get_season_archives(
        &self,
        mid: u64,
        season_id: u64,
        pn: u64,
    ) -> Result<Page<TrackItem>> {
        let data = self
            .get(
                API,
                "/x/space/fav/season/list",
                json!({
                    "season_id": season_id, "pn": pn, "ps": 30, "web_location": "333.1387"
                }),
                false,
            )
            .await?;
        Ok(track_page(
            &data["medias"],
            number(&data["info"]["media_count"]),
            pn,
            30,
            Some(mid),
        ))
    }

    pub async fn get_series_archives(
        &self,
        mid: u64,
        series_id: u64,
        pn: u64,
    ) -> Result<Page<TrackItem>> {
        let data = self
            .get(
                API,
                "/x/series/archives",
                json!({
                    "mid": mid, "series_id": series_id, "sort": "desc", "pn": pn, "ps": 30
                }),
                false,
            )
            .await?;
        Ok(track_page(
            &data["archives"],
            number(&data["page"]["total"]),
            pn,
            30,
            Some(mid),
        ))
    }

    pub async fn get_later_list(&self) -> Result<Page<TrackItem>> {
        self.get_later_list_page(1).await
    }

    pub async fn get_later_list_page(&self, pn: u64) -> Result<Page<TrackItem>> {
        let data = self
            .get(
                API,
                "/x/v2/history/toview/web",
                json!({ "pn": pn, "ps": 20 }),
                true,
            )
            .await?;
        Ok(track_page(
            &data["list"],
            number(&data["count"]),
            pn,
            20,
            None,
        ))
    }

    pub async fn delete_later(&self, aid: u64) -> Result<()> {
        self.post(API, "/x/v2/history/toview/del", json!({ "aid": aid }), None)
            .await?;
        Ok(())
    }

    pub async fn get_history(
        &self,
        max: u64,
        view_at: u64,
        keyword: Option<&str>,
    ) -> Result<Page<HistoryItem>> {
        if let Some(keyword) = keyword.filter(|keyword| !keyword.trim().is_empty()) {
            return self.search_history(keyword, max.max(1), view_at).await;
        }
        let data = self.get(API, "/x/web-interface/history/cursor",
            json!({ "max": max, "view_at": view_at, "business": "archive", "type": "archive", "ps": 30 }), false).await?;
        let items = list(&data["list"])
            .iter()
            .map(history_item)
            .collect::<Vec<_>>();
        let next = number(&data["cursor"]["max"]).zip(number(&data["cursor"]["view_at"]));
        let next = next.filter(|(max, time)| *max > 0 && *time > 0 && !items.is_empty());
        Ok(Page {
            items,
            total: None,
            has_more: Some(next.is_some()),
            next_offset: next.map(|(id, time)| format!("{id}:{time}")),
        })
    }

    pub async fn search_history(
        &self,
        keyword: &str,
        pn: u64,
        view_at: u64,
    ) -> Result<Page<HistoryItem>> {
        let data = self.get(API, "/x/web-interface/history/search",
            json!({ "keyword": keyword, "pn": pn, "business": "archive", "add_time_end": view_at }), false).await?;
        let pages = number(&data["page"]["total"]);
        let has_more = data["has_more"]
            .as_bool()
            .or_else(|| pages.map(|count| pn < count));
        Ok(Page {
            items: list(&data["list"]).iter().map(history_item).collect(),
            total: None,
            has_more,
            next_offset: (has_more == Some(true)).then(|| pn.saturating_add(1).to_string()),
        })
    }

    pub async fn delete_history(&self, kid: &str) -> Result<()> {
        self.post(API, "/x/v2/history/delete", json!({ "kid": kid }), None)
            .await?;
        Ok(())
    }

    pub async fn clear_history(&self) -> Result<()> {
        self.post(API, "/x/v2/history/clear", Value::Null, None)
            .await?;
        Ok(())
    }

    pub async fn get_followings(
        &self,
        mid: u64,
        pn: u64,
        name_filter: Option<&str>,
    ) -> Result<Page<FollowingUser>> {
        let path = if name_filter.is_some_and(|name| !name.is_empty()) {
            "/x/relation/followings/search"
        } else {
            "/x/relation/followings"
        };
        let data = self
            .get(
                API,
                path,
                json!({ "vmid": mid, "pn": pn, "ps": 50, "name": name_filter }),
                false,
            )
            .await?;
        let total = number(&data["total"]);
        Ok(Page {
            items: decode(Value::Array(list(&data["list"]).to_vec()))?,
            total,
            has_more: total.map(|count| pn.saturating_mul(50) < count),
            next_offset: None,
        })
    }

    pub async fn modify_relation(&self, fid: u64, act: u32) -> Result<()> {
        self.post(
            API,
            "/x/relation/modify",
            json!({ "fid": fid, "act": act, "re_src": 11 }),
            None,
        )
        .await?;
        Ok(())
    }

    pub async fn get_dynamic_feed(&self, offset: Option<&str>) -> Result<Page<DynamicItem>> {
        let data = self.get(API, "/x/polymer/web-dynamic/v1/feed/all", json!({
            "type": "video", "offset": offset, "timezone_offset": -480, "platform": "web", "web_location": "333.1365"
        }), false).await?;
        let items = list(&data["items"])
            .iter()
            .map(|item| {
                let module = &item["modules"];
                let archive = &module["module_dynamic"]["major"]["archive"];
                let mut track = video_track(archive);
                if let Some(track) = &mut track {
                    track.artist = strip_html_tags(field(&module["module_author"], &["name"]));
                    track.artist_mid = number(&module["module_author"]["mid"]);
                }
                DynamicItem {
                    id: field(item, &["id_str"]).to_owned(),
                    track,
                    raw: sanitize_fields(item.clone()),
                }
            })
            .collect();
        Ok(Page {
            items,
            total: None,
            has_more: data["has_more"].as_bool(),
            next_offset: data["offset"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
        })
    }

    pub async fn get_user_info(&self, mid: u64) -> Result<UserInfo> {
        decode(
            self.get(API, "/x/space/wbi/acc/info", json!({ "mid": mid }), true)
                .await?,
        )
    }

    pub async fn get_user_videos(
        &self,
        mid: u64,
        pn: u64,
        keyword: Option<&str>,
    ) -> Result<Page<TrackItem>> {
        let data = self
            .get(
                API,
                "/x/space/wbi/arc/search",
                json!({
                    "mid": mid, "pn": pn, "ps": 30, "order": "pubdate", "keyword": keyword
                }),
                true,
            )
            .await?;
        Ok(track_page(
            &data["list"]["vlist"],
            number(&data["page"]["count"]),
            pn,
            30,
            Some(mid),
        ))
    }

    pub async fn get_user_seasons_series(&self, mid: u64, pn: u64) -> Result<Page<FavFolder>> {
        let data = self
            .get(
                API,
                "/x/polymer/web-space/seasons_series_list",
                json!({
                    "mid": mid, "page_num": pn, "page_size": 20, "web_location": "333.1387"
                }),
                false,
            )
            .await?;
        let data = &data["items_lists"];
        let mut items = Vec::new();
        for (key, kind, id_key) in [
            ("seasons_list", 21, "season_id"),
            ("series_list", 0, "series_id"),
        ] {
            for row in list(&data[key]) {
                let mut meta = row["meta"].clone();
                if let Some(fields) = meta.as_object_mut() {
                    let id = fields.get(id_key).cloned().unwrap_or_default();
                    let count = fields.get("total").cloned().unwrap_or_default();
                    fields.insert("id".into(), json!(number(&id).unwrap_or_default()));
                    fields.insert(
                        "media_count".into(),
                        json!(number(&count).unwrap_or_default()),
                    );
                    fields.insert("type".into(), json!(kind));
                    if !fields.contains_key("title")
                        && let Some(name) = fields.get("name").cloned()
                    {
                        fields.insert("title".into(), name);
                    }
                    fields.insert("archives".into(), row["archives"].clone());
                }
                items.push(decode(meta)?);
            }
        }
        let total = number(&data["page"]["total"]);
        Ok(Page {
            items,
            total,
            has_more: total.map(|count| pn.saturating_mul(20) < count),
            next_offset: None,
        })
    }
}

fn duration(value: &Value) -> f64 {
    value
        .as_f64()
        .or_else(|| {
            value.as_str()?.split(':').try_fold(0.0, |seconds, part| {
                Some(seconds * 60.0 + part.parse::<f64>().ok()?)
            })
        })
        .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
        .unwrap_or_default()
}

fn cover_url(value: &Value) -> String {
    let cover = field(value, &["pic", "cover"]);
    if cover.starts_with("//") {
        format!("https:{cover}")
    } else {
        cover.to_owned()
    }
}

fn video_track(value: &Value) -> Option<TrackItem> {
    let related = &value["related_archive"];
    let bvid = [value, &value["history"], related]
        .into_iter()
        .map(|row| field(row, &["bvid"]))
        .find(|id| !id.is_empty())?
        .to_owned();
    let aid = number(&value["aid"])
        .or_else(|| number(&value["id"]))
        .or_else(|| number(&value["history"]["oid"]))
        .or_else(|| number(&related["aid"]))
        .unwrap_or_default();
    let cid = number(&value["cid"])
        .or_else(|| number(&value["history"]["cid"]))
        .or_else(|| number(&related["cid"]))
        .unwrap_or_default();
    let owner = [&value["owner"], &value["upper"], &value["author"]]
        .into_iter()
        .find(|owner| owner.is_object())
        .unwrap_or(&Value::Null);
    let title = field(value, &["title", "archive_title", "music_title"]);
    let artist = field(value, &["author", "author_name", "uname"]);
    let part = field(&value["history"], &["part"]);
    Some(TrackItem {
        id: format!("{bvid}:{cid}"),
        title: strip_html_tags(if title.is_empty() {
            field(related, &["title"])
        } else {
            title
        }),
        artist: strip_html_tags(if artist.is_empty() {
            field(owner, &["name", "uname"])
        } else {
            artist
        }),
        artist_mid: number(&owner["mid"])
            .or_else(|| number(&value["mid"]))
            .or_else(|| number(&value["author_mid"])),
        cover: cover_url(value),
        duration_secs: ["duration", "length", "duration_text"]
            .into_iter()
            .map(|key| duration(&value[key]))
            .find(|d| *d > 0.0)
            .unwrap_or_else(|| duration(&related["duration"])),
        source: TrackSource::BiliVideo {
            bvid,
            aid,
            cid,
            part_title: (!part.is_empty()).then(|| strip_html_tags(part)),
        },
    })
}

fn audio_track(data: &Value, sid: u64) -> TrackItem {
    TrackItem {
        id: format!("au:{sid}"),
        title: strip_html_tags(field(data, &["title"])),
        artist: strip_html_tags(field(data, &["author", "uname"])),
        artist_mid: number(&data["uid"]),
        cover: cover_url(data),
        duration_secs: duration(&data["duration"]),
        source: TrackSource::BiliAudio { sid },
    }
}

fn tracks(data: &Value) -> Vec<TrackItem> {
    list(data)
        .iter()
        .filter_map(|item| {
            if number(&item["type"]) == Some(12) {
                let sid = number(&item["id"])?;
                let mut track = audio_track(item, sid);
                if track.artist.is_empty() {
                    track.artist = strip_html_tags(field(&item["upper"], &["name"]));
                    track.artist_mid = number(&item["upper"]["mid"]);
                }
                Some(track)
            } else {
                video_track(item)
            }
        })
        .collect()
}

fn track_page(
    data: &Value,
    total: Option<u64>,
    page: u64,
    size: u64,
    artist_mid: Option<u64>,
) -> Page<TrackItem> {
    let mut items = tracks(data);
    for track in &mut items {
        track.artist_mid = track.artist_mid.or(artist_mid);
    }
    Page {
        items,
        total,
        has_more: total.map(|count| page.saturating_mul(size) < count),
        next_offset: None,
    }
}

fn check_search_data(data: &Value) -> Result<()> {
    ensure!(
        !data["v_voucher"]
            .as_str()
            .is_some_and(|voucher| !voucher.is_empty()),
        "Bilibili search requires additional verification"
    );
    ensure!(data.is_object(), "Bilibili search did not return data");
    Ok(())
}

fn search_page<T>(data: &Value, items: Vec<T>) -> SearchResult<T> {
    SearchResult {
        items,
        page: number(&data["page"]).unwrap_or_default(),
        page_size: number(&data["pagesize"]).unwrap_or_default(),
        total: number(&data["numResults"]).unwrap_or_default(),
        pages: number(&data["numPages"]).unwrap_or_default(),
    }
}

fn rank_item(data: &Value) -> RankItem {
    RankItem {
        track: video_track(data),
        title: strip_html_tags(field(data, &["music_title", "archive_title"])),
        author: strip_html_tags(field(data, &["author"])),
        cover: cover_url(data),
        score: data["score"].as_f64().unwrap_or_default(),
        raw: sanitize_fields(data.clone()),
    }
}

fn history_item(data: &Value) -> HistoryItem {
    let history = &data["history"];
    let business = field(history, &["business"]);
    let kid = number(&data["kid"])
        .or_else(|| number(&history["oid"]))
        .map(|id| format!("{business}_{id}"))
        .unwrap_or_default();
    HistoryItem {
        track: video_track(data),
        title: strip_html_tags(field(data, &["title"])),
        kid,
        view_at: number(&data["view_at"]).unwrap_or_default(),
        progress: data["progress"].as_i64().unwrap_or_default(),
        raw: sanitize_fields(data.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_supported_audio_and_registers_audio_hosts_only() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let client = BiliClient::new(dir.path().join("cookies.json"), None)?;
        let payload = json!({"dash":{
            "audio":[{"id":30216,"bandwidth":64,"baseUrl":"https://a.fixture.example/a.m4s"},
                {"id":30280,"bandwidth":128,"base_url":"https://b.fixture.example/b.m4s",
                 "backup_url":["https://c.fixture.example/b.m4s"]}],
            "flac":{"audio":{"id":30251,"baseUrl":"https://d.fixture.example/c.m4s"}},
            "dolby":{"audio":[{"id":30250,"baseUrl":"https://e.fixture.example/c.m4s"}]},
            "video":[{"baseUrl":"https://video.fixture.example/video.m4s"}]
        }});
        let auto = client.playurl_info(&payload, AudioQuality::Auto)?;
        let lossless = client.playurl_info(&payload, AudioQuality::Lossless)?;
        let high = client.playurl_info(&payload, AudioQuality::High)?;
        let low = client.playurl_info(&payload, AudioQuality::Low)?;
        assert_eq!(
            auto.streams.iter().map(|s| s.id).collect::<Vec<_>>(),
            [30251, 30280, 30216, 30250]
        );
        assert_eq!(lossless.streams[1].id, 30250);
        assert_eq!(high.urls.len(), 2);
        assert_eq!(low.streams[0].id, 30216);
        assert!(
            auto.streams
                .iter()
                .flat_map(|stream| &stream.urls)
                .all(|url| client.is_media_url_allowed(url))
        );
        assert!(
            !client.is_media_url_allowed(&Url::parse("https://video.fixture.example/video.m4s")?)
        );
        Ok(())
    }

    #[tokio::test]
    async fn logs_out_locally_when_the_server_is_unreachable() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("cookies.json");
        let client = BiliClient::new(path.clone(), None)?;
        client.set_cookie("bili_jct", "fixture", None)?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let proxy = reqwest::Proxy::all(format!("http://{}", listener.local_addr()?))?;
        client.clients.write().unwrap().0 = Client::builder()
            .proxy(proxy)
            .timeout(Duration::from_secs(3))
            .build()?;
        let server = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let (mut socket, _) = listener.accept().await?;
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(socket.read_u8().await?);
            }
            assert!(request.starts_with(b"CONNECT passport.bilibili.com:443 "));
            socket
                .write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n")
                .await?;
            Ok::<_, std::io::Error>(())
        });
        client.logout().await?;
        tokio::time::timeout(Duration::from_secs(3), server).await???;
        assert!(client.get_cookie("bili_jct").is_none());
        assert!(
            BiliClient::new(path, None)?
                .get_cookie("bili_jct")
                .is_none()
        );
        Ok(())
    }
}
