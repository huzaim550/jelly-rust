//! Item queries and projection into `BaseItemDto`.

use crate::{Database, DbError, Result};
use jf_core::{
    BaseItemDto, BaseItemKind, CollectionType, ExternalUrlRef, MediaType, PersonRef,
    QueryResult, UserItemDataDto,
};
use serde::{Deserialize, Serialize};
use sqlx::AssertSqlSafe;
use sqlx::Row;

/// Sort fields accepted by `/Items`, matching Jellyfin's `SortBy` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortBy {
    SortName,
    Name,
    PremiereDate,
    DateCreated,
    ProductionYear,
    CommunityRating,
    RuntimeTicks,
    Random,
}

impl SortBy {
    /// Map a Jellyfin sort key onto a SQL ordering.
    ///
    /// Returns a fragment to interpolate into an ORDER BY. Values are matched
    /// against a fixed table rather than interpolated from user input, so this
    /// cannot inject SQL.
    pub fn sql(self) -> &'static str {
        match self {
            SortBy::SortName => "i.sort_name COLLATE NOCASE",
            SortBy::Name => "i.name COLLATE NOCASE",
            SortBy::PremiereDate => "i.premiere_date",
            SortBy::DateCreated => "i.date_created",
            SortBy::ProductionYear => "i.production_year",
            SortBy::CommunityRating => "i.community_rating",
            SortBy::RuntimeTicks => "i.run_time_ticks",
            SortBy::Random => "RANDOM()",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw {
            "SortName" => SortBy::SortName,
            "Name" => SortBy::Name,
            "PremiereDate" => SortBy::PremiereDate,
            "DateCreated" => SortBy::DateCreated,
            "ProductionYear" => SortBy::ProductionYear,
            "CommunityRating" => SortBy::CommunityRating,
            "RuntimeTicks" | "Runtime" => SortBy::RuntimeTicks,
            "Random" => SortBy::Random,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum SortOrder {
    #[default]
    Ascending,
    Descending,
}

/// Filters accepted by `/Items`.
#[derive(Debug, Clone, Default)]
pub struct ItemQuery {
    pub parent_id: Option<String>,
    pub library_id: Option<String>,
    pub item_types: Vec<BaseItemKind>,
    pub media_types: Vec<MediaType>,
    pub search: Option<String>,
    pub sort_by: Option<SortBy>,
    pub sort_order: SortOrder,
    pub start_index: usize,
    pub limit: usize,
    pub recursive: bool,
    pub include_folders: bool,
    pub genres: Vec<String>,
    pub years: Vec<i32>,
    /// Restrict to titles this user has started but not finished.
    pub in_progress_only: bool,
    pub played_only: bool,
    pub unplayed_only: bool,
    pub favorites_only: bool,
}

/// Fields requested via the `Fields` query parameter.
///
/// Fields are opt-in to keep responses small; the UI asks for what it needs.
#[derive(Debug, Clone, Default)]
pub struct FieldSelection {
    pub overview: bool,
    pub genres: bool,
    pub tags: bool,
    pub media_sources: bool,
    pub people: bool,
    pub date_created: bool,
    pub sort_name: bool,
    pub overview_only: bool,
    pub images: bool,
}

impl FieldSelection {
    /// Parse Jellyfin's comma-separated `Fields` parameter.
    pub fn parse(raw: Option<&str>) -> Self {
        let Some(raw) = raw else { return Self::default() };
        let set: Vec<String> = raw.split(',').map(|s| s.trim().to_lowercase()).collect();

        Self {
            overview: set.iter().any(|f| f == "overview"),
            genres: set.iter().any(|f| f == "genres"),
            tags: set.iter().any(|f| f == "tags"),
            media_sources: set.iter().any(|f| f == "mediasources" || f == "mediastreams"),
            people: set.iter().any(|f| f == "people" || f == "persons"),
            date_created: set.iter().any(|f| f == "datecreated"),
            sort_name: set.iter().any(|f| f == "sortname"),
            images: true,
            overview_only: false,
        }
    }

    /// All fields, for detail responses where the payload size is less
    /// important than a complete object.
    pub fn all() -> Self {
        Self {
            overview: true,
            genres: true,
            tags: true,
            media_sources: true,
            people: true,
            date_created: true,
            sort_name: true,
            overview_only: false,
            images: true,
        }
    }
}

impl Database {
    /// Run an item query.
    pub async fn query_items(
        &self,
        user_id: Option<&str>,
        q: &ItemQuery,
        fields: &FieldSelection,
        server_id: &str,
    ) -> Result<QueryResult<BaseItemDto>> {
        let mut where_sql: Vec<String> = Vec::new();
        let mut args: Vec<String> = Vec::new();

        if !q.include_folders {
            where_sql.push("i.is_folder = 0".to_owned());
        }
        if let Some(parent) = &q.parent_id {
            if q.recursive {
                // Recursive descendant search via the parent chain.
                where_sql.push(
                    "(i.id = ? OR i.parent_id = ? OR EXISTS (
                        WITH RECURSIVE ancestors(id) AS (
                            SELECT id FROM items WHERE id = ?
                            UNION ALL
                            SELECT it.id FROM items it
                            JOIN ancestors a ON it.parent_id = a.id
                        )
                        SELECT 1 FROM ancestors WHERE ancestors.id = i.id
                    ))".to_owned(),
                );
                args.push(parent.clone());
                args.push(parent.clone());
                args.push(parent.clone());
            } else {
                where_sql.push("i.parent_id = ?".to_owned());
                args.push(parent.clone());
            }
        }
        if let Some(lib) = &q.library_id {
            where_sql.push("i.library_id = ?".to_owned());
            args.push(lib.clone());
        }
        if !q.item_types.is_empty() {
            let names: Vec<&str> = q.item_types.iter().map(kind_name).collect();
            where_sql.push(format!("i.kind IN ({})", placeholders(names.len())));
            args.extend(names.into_iter().map(str::to_owned));
        }
        if !q.media_types.is_empty() {
            let names: Vec<&str> = q.media_types.iter().map(media_name).collect();
            where_sql.push(format!("i.media_type IN ({})", placeholders(names.len())));
            args.extend(names.into_iter().map(str::to_owned));
        }
        if let Some(term) = q.search.as_ref().filter(|t| !t.trim().is_empty()) {
            where_sql.push("(i.name LIKE ? OR i.overview LIKE ? OR i.sort_name LIKE ?)".to_owned());
            let pattern = format!("%{}%", escape_like(term.trim()));
            args.push(pattern.clone());
            args.push(pattern.clone());
            args.push(pattern);
        }
        if !q.genres.is_empty() {
            where_sql.push(format!("i.genres_json LIKE ({})", placeholders(q.genres.len())));
            args.extend(q.genres.iter().map(|g| format!("%\"{g}\"%")));
        }
        if !q.years.is_empty() {
            where_sql.push(format!("i.production_year IN ({})", placeholders(q.years.len())));
            args.extend(q.years.iter().map(|y| y.to_string()));
        }

        // Per-user playback filters.
        if let Some(uid) = user_id {
            if q.in_progress_only {
                where_sql.push(
                    "EXISTS (SELECT 1 FROM user_data ud WHERE ud.item_id = i.id
                     AND ud.user_id = ? AND ud.playback_position_ticks > 0 AND ud.played = 0)".to_owned(),
                );
                args.push(uid.to_owned());
            }
            if q.played_only {
                where_sql.push(
                    "EXISTS (SELECT 1 FROM user_data ud WHERE ud.item_id = i.id
                     AND ud.user_id = ? AND ud.played = 1)".to_owned(),
                );
                args.push(uid.to_owned());
            }
            if q.unplayed_only {
                where_sql.push(
                    "NOT EXISTS (SELECT 1 FROM user_data ud WHERE ud.item_id = i.id
                     AND ud.user_id = ? AND ud.played = 1)".to_owned(),
                );
                args.push(uid.to_owned());
            }
            if q.favorites_only {
                where_sql.push(
                    "EXISTS (SELECT 1 FROM user_data ud WHERE ud.item_id = i.id
                     AND ud.user_id = ? AND ud.is_favorite = 1)".to_owned(),
                );
                args.push(uid.to_owned());
            }
        }

        let where_clause = if where_sql.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", where_sql.join(" AND "))
        };

        let order = q.sort_by.unwrap_or(SortBy::SortName).sql();
        let direction = match q.sort_order {
            SortOrder::Ascending => "ASC",
            SortOrder::Descending => "DESC",
        };

        let limit = q.limit.clamp(1, 5000);
        let offset = q.start_index;

        // Only fixed, code-generated fragments are interpolated into these
        // strings. Every user-controlled value goes through `args` and is bound
        // as a parameter, so this is safe to assert.
        let rows_sql = AssertSqlSafe(format!(
            "SELECT i.* FROM items i {where_clause}
             ORDER BY {order} {direction}
             LIMIT {limit} OFFSET {offset}"
        ));

        // Total ignores LIMIT/OFFSET so clients can paginate.
        let count_sql =
            AssertSqlSafe(format!("SELECT COUNT(*) FROM items i {where_clause}"));

        let mut count_query = sqlx::query_scalar(count_sql);
        let mut rows_query = sqlx::query(rows_sql);
        for arg in &args {
            count_query = count_query.bind(arg.as_str());
            rows_query = rows_query.bind(arg.as_str());
        }

        let total: i64 = count_query.fetch_one(self.pool()).await?;
        let rows = rows_query.fetch_all(self.pool()).await?;

        let mut items = Vec::with_capacity(rows.len());
        for row in &rows {
            let dto = self.row_to_item(row, fields, server_id).await?;
            // Attach this user's playback state.
            let dto = match user_id {
                Some(uid) => self.attach_user_data(dto, uid).await?,
                None => dto,
            };
            items.push(dto);
        }

        Ok(QueryResult {
            items,
            total_record_count: total as usize,
            start_index: offset,
        })
    }

    /// Fetch one item, optionally with per-user state.
    pub async fn item_by_id(
        &self,
        id: &str,
        user_id: Option<&str>,
        server_id: &str,
    ) -> Result<Option<BaseItemDto>> {
        let row = sqlx::query("SELECT * FROM items WHERE id = ?")
            .bind(id)
            .fetch_optional(self.pool())
            .await?;

        let Some(row) = row else { return Ok(None) };
        let dto = self.row_to_item(&row, &FieldSelection::all(), server_id).await?;

        Ok(Some(match user_id {
            Some(uid) => self.attach_user_data(dto, uid).await?,
            None => dto,
        }))
    }

    /// Read playback progress for an item and user.
    pub async fn user_item_data(&self, user_id: &str, item_id: &str) -> Result<UserItemDataDto> {
        let row = sqlx::query(
            "SELECT * FROM user_data WHERE user_id = ? AND item_id = ?",
        )
        .bind(user_id)
        .bind(item_id)
        .fetch_optional(self.pool())
        .await?;

        let Some(row) = row else { return Ok(UserItemDataDto::default()) };

        Ok(UserItemDataDto {
            playback_position_ticks: row.get("playback_position_ticks"),
            play_count: row.get("play_count"),
            is_favorite: row.get::<i64, _>("is_favorite") != 0,
            played: row.get::<i64, _>("played") != 0,
            last_played_date: row.get("last_played_date"),
            unplayed_item_count: 0,
        })
    }

    /// Record playback progress.
    ///
    /// `played` is derived rather than trusted: a position within the final
    /// 2% counts as finished, which is how Jellyfin behaves and stops clients
    /// from having to decide.
    pub async fn set_progress(
        &self,
        user_id: &str,
        item_id: &str,
        position_ticks: i64,
        is_favorite: Option<bool>,
    ) -> Result<UserItemDataDto> {
        let runtime: Option<i64> =
            sqlx::query_scalar("SELECT run_time_ticks FROM items WHERE id = ?")
                .bind(item_id)
                .fetch_optional(self.pool())
                .await?;

        let played = match runtime {
            Some(rt) if rt > 0 => position_ticks as f64 >= rt as f64 * 0.98,
            _ => false,
        };

        // Advance the resume point only: rewinding should not move it, and a
        // position past the end should not either.
        let now = crate::users::now_iso();
        sqlx::query(
            "INSERT INTO user_data
                (user_id, item_id, playback_position_ticks, play_count, is_favorite, played, last_played_date)
             VALUES (?, ?, ?, ?, COALESCE(?, 0), ?, ?)
             ON CONFLICT(user_id, item_id) DO UPDATE SET
                playback_position_ticks = MAX(excluded.playback_position_ticks, user_data.playback_position_ticks),
                is_favorite = COALESCE(?, user_data.is_favorite),
                played = CASE WHEN excluded.played THEN 1 ELSE user_data.played END,
                last_played_date = CASE WHEN excluded.played AND user_data.played = 0
                                        THEN excluded.last_played_date ELSE user_data.last_played_date END,
                play_count = user_data.play_count + CASE WHEN excluded.played AND user_data.played = 0
                                        THEN 1 ELSE 0 END",
        )
        .bind(user_id)
        .bind(item_id)
        .bind(position_ticks.max(0))
        .bind(if played { 1i64 } else { 0 })
        .bind(is_favorite)
        .bind(if played { 1i64 } else { 0 })
        .bind(&now)
        .bind(is_favorite)
        .execute(self.pool())
        .await?;

        self.user_item_data(user_id, item_id).await
    }

    /// Toggle or set the favourite flag.
    pub async fn set_favorite(&self, user_id: &str, item_id: &str, favorite: bool) -> Result<()> {
        sqlx::query(
            "INSERT INTO user_data (user_id, item_id, is_favorite) VALUES (?, ?, ?)
             ON CONFLICT(user_id, item_id) DO UPDATE SET is_favorite = excluded.is_favorite",
        )
        .bind(user_id)
        .bind(item_id)
        .bind(if favorite { 1i64 } else { 0 })
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// The root folder item for each library, used to build `/Views`.
    ///
    /// Resolved by `library_id` rather than by reusing the library row's id:
    /// the two ids are allocated independently, so looking the root up by id
    /// silently returns nothing.
    pub async fn library_root_items(
        &self,
        library_ids: &[String],
        user_id: &str,
        server_id: &str,
    ) -> Result<Vec<BaseItemDto>> {
        if library_ids.is_empty() {
            return Ok(Vec::new());
        }

        let holes = std::iter::repeat_n("?", library_ids.len())
            .collect::<Vec<_>>()
            .join(", ");

        // Only the generated placeholder list is interpolated; every id is
        // bound as a parameter.
        let sql = AssertSqlSafe(format!(
            "SELECT * FROM items WHERE library_id IN ({holes}) AND is_folder = 1 \
             ORDER BY name COLLATE NOCASE"
        ));

        let mut query = sqlx::query(sql);
        for id in library_ids {
            query = query.bind(id);
        }

        let rows = query.fetch_all(self.pool()).await?;
        let fields = FieldSelection::default();

        let mut items = Vec::with_capacity(rows.len());
        for row in &rows {
            let mut item = self.row_to_item(row, &fields, server_id).await?;
            item.user_data = Some(self.user_item_data(user_id, &item.id).await?);
            items.push(item);
        }
        Ok(items)
    }

    /// Item ids a user is allowed to see, honouring their folder policy.
    ///
    /// A policy with `enable_all_folders` set permits everything; otherwise
    /// only the listed library ids are readable.
    pub async fn visible_library_ids(
        &self,
        user_id: &str,
        policy: &jf_core::UserPolicy,
    ) -> Result<Vec<String>> {
        let all: Vec<String> = sqlx::query_scalar("SELECT id FROM libraries WHERE enabled = 1")
            .fetch_all(self.pool())
            .await?;

        if policy.enable_all_folders {
            return Ok(all);
        }
        Ok(all
            .into_iter()
            .filter(|id| policy.enabled_folders.iter().any(|f| f == id))
            .collect())
    }

    /// Direct-stream path for an item, if it exists on disk.
    pub async fn item_media_path(&self, item_id: &str) -> Result<Option<String>> {
        Ok(sqlx::query_scalar("SELECT path FROM items WHERE id = ?")
            .bind(item_id)
            .fetch_optional(self.pool())
            .await?)
    }

    /// Image path for an item, preferring a sidecar over a generated thumbnail.
    pub async fn item_image_path(
        &self,
        item_id: &str,
        image_type: &str,
        index: u32,
    ) -> Result<Option<String>> {
        Ok(sqlx::query_scalar(
            "SELECT path FROM images WHERE item_id = ? AND image_type = ? AND index_num = ?",
        )
        .bind(item_id)
        .bind(image_type)
        .bind(index as i64)
        .fetch_optional(self.pool())
        .await?)
    }

    async fn attach_user_data(
        &self,
        mut dto: BaseItemDto,
        user_id: &str,
    ) -> Result<BaseItemDto> {
        // Clients read `UserData` off each item for resume positions, play
        // counts, and the favourite flag, so it is filled in here rather than
        // requiring a second request per item.
        dto.user_data = Some(self.user_item_data(user_id, &dto.id).await?);
        Ok(dto)
    }

    async fn row_to_item(
        &self,
        row: &sqlx::sqlite::SqliteRow,
        fields: &FieldSelection,
        server_id: &str,
    ) -> Result<BaseItemDto> {
        let kind_raw: String = row.get("kind");
        let kind = parse_kind(&kind_raw);

        let is_folder: i64 = row.get("is_folder");
        let media_type = row.get::<Option<String>, _>("media_type").and_then(|m| parse_media_type(&m));
        let container = row.get::<Option<String>, _>("container");
        let library_id: String = row.get("library_id");

        // Folder views carry the library's collection type.
        let collection_type = if is_folder == 1 {
            sqlx::query_scalar::<_, String>("SELECT collection_type FROM libraries WHERE id = ?")
                .bind(&library_id)
                .fetch_optional(self.pool())
                .await?
                .and_then(|t| parse_collection_type(&t))
        } else {
            None
        };

        let image_tags: std::collections::BTreeMap<String, String> =
            crate::json_column(&row.get::<String, _>("image_tags_json"), "items", "image_tags_json")
                .unwrap_or_default();

        let genres: Vec<String> = if fields.genres {
            crate::json_column(&row.get::<String, _>("genres_json"), "items", "genres_json")
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        let tags: Vec<String> = if fields.tags {
            crate::json_column(&row.get::<String, _>("tags_json"), "items", "tags_json")
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        let people: Vec<PersonRef> = if fields.people {
            crate::json_column(&row.get::<String, _>("people_json"), "items", "people_json")
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        let taglines: Vec<String> =
            crate::json_column(&row.get::<String, _>("taglines_json"), "items", "taglines_json")
                .unwrap_or_default();

        let backdrop: Vec<String> =
            crate::json_column(&row.get::<String, _>("backdrop_json"), "items", "backdrop_json")
                .unwrap_or_default();

        let id = row.get::<String, _>("id");
        let path = row.get::<String, _>("path");

        let mut external_urls = Vec::new();
        if fields.overview {
            let ids: Vec<(String, String)> = sqlx::query(
                "SELECT provider, value FROM item_external_ids WHERE item_id = ?",
            )
            .bind(&id)
            .fetch_all(self.pool())
            .await?
            .into_iter()
            .map(|r| (r.get::<String, _>("provider"), r.get::<String, _>("value")))
            .collect();

            for (provider, value) in ids {
                if let Some(url) = provider_url(&provider, &value) {
                    external_urls.push(ExternalUrlRef::new(provider, url));
                }
            }
        }

        Ok(BaseItemDto {
            name: row.get("name"),
            server_id: server_id.to_owned(),
            id,
            kind,
            collection_type,
            is_folder: is_folder == 1,
            media_type,
            path,
            size: row.get("size"),
            container,
            date_created: fields.date_created.then(|| row.get("date_created")).flatten(),
            production_year: row.get("production_year"),
            index_number: row.get("index_number"),
            parent_index_number: row.get("parent_index_number"),
            overview: if fields.overview { row.get("overview") } else { None },
            genres,
            taglines,
            run_time_ticks: row.get("run_time_ticks"),
            official_rating: row.get("official_rating"),
            community_rating: row.get("community_rating"),
            critic_rating: row.get("critic_rating"),
            status: row.get("status"),
            parent_id: row.get("parent_id"),
            sort_name: fields.sort_name.then(|| row.get("sort_name")).flatten(),
            series_name: row.get("series_name"),
            season_name: row.get("season_name"),
            episode_title: row.get("episode_title"),
            series_id: row.get("series_id"),
            series_primary_image_tag: None,
            season_id: row.get("season_id"),
            premiere_date: row.get("premiere_date"),
            people,
            tags,
            album: row.get("album"),
            album_id: row.get("album_id"),
            album_artist: row.get("album_artist"),
            artists: Vec::new(),
            album_primary_image_tag: None,
            image_tags: if image_tags.is_empty() { None } else { Some(image_tags) },
            backdrop_image_tags: backdrop,
            parent_backdrop_item_id: None,
            parent_thumb_item_id: None,
            series_thumb_image_tag: None,
            can_delete: false,
            can_download: true,
            is_live_stream: false,
            external_urls,
            user_data: None,
        })
    }
}

/// Render a `?` placeholder list for an IN clause.
fn placeholders(n: usize) -> String {
    std::iter::repeat_n("?", n).collect::<Vec<_>>().join(", ")
}

/// Escape LIKE wildcards so a search for "100%" does not match everything.
fn escape_like(term: &str) -> String {
    term.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

fn kind_name(kind: &BaseItemKind) -> &'static str {
    match kind {
        BaseItemKind::AggregateFolder => "AggregateFolder",
        BaseItemKind::UserRootFolder => "UserRootFolder",
        BaseItemKind::UserView => "UserView",
        BaseItemKind::CollectionFolder => "CollectionFolder",
        BaseItemKind::Folder => "Folder",
        BaseItemKind::Movie => "Movie",
        BaseItemKind::Series => "Series",
        BaseItemKind::Season => "Season",
        BaseItemKind::Episode => "Episode",
        BaseItemKind::BoxSet => "BoxSet",
        BaseItemKind::Video => "Video",
        BaseItemKind::MusicAlbum => "MusicAlbum",
        BaseItemKind::MusicArtist => "MusicArtist",
        BaseItemKind::Audio => "Audio",
        BaseItemKind::Playlist => "Playlist",
        BaseItemKind::PhotoAlbum => "PhotoAlbum",
        BaseItemKind::Recording => "Recording",
        BaseItemKind::Program => "Program",
        BaseItemKind::TvChannel => "TvChannel",
        BaseItemKind::Person => "Person",
        BaseItemKind::Studio => "Studio",
        BaseItemKind::Genre => "Genre",
        BaseItemKind::MusicGenre => "MusicGenre",
    }
}

/// Strict kind parser for query filters.
///
/// Unlike [`parse_kind`] this returns `None` for unknown names, so a client
/// typo narrows to nothing rather than silently matching every folder.
pub fn try_parse_kind(raw: &str) -> Option<BaseItemKind> {
    const ALL: &[BaseItemKind] = &[
        BaseItemKind::AggregateFolder,
        BaseItemKind::UserRootFolder,
        BaseItemKind::UserView,
        BaseItemKind::CollectionFolder,
        BaseItemKind::Folder,
        BaseItemKind::Movie,
        BaseItemKind::Series,
        BaseItemKind::Season,
        BaseItemKind::Episode,
        BaseItemKind::BoxSet,
        BaseItemKind::Video,
        BaseItemKind::MusicAlbum,
        BaseItemKind::MusicArtist,
        BaseItemKind::Audio,
        BaseItemKind::Playlist,
        BaseItemKind::PhotoAlbum,
        BaseItemKind::Recording,
        BaseItemKind::Program,
        BaseItemKind::TvChannel,
        BaseItemKind::Person,
        BaseItemKind::Studio,
        BaseItemKind::Genre,
        BaseItemKind::MusicGenre,
    ];
    ALL.iter().copied().find(|k| kind_name(k) == raw)
}

/// Parse a kind string back into the enum.
pub fn parse_kind(raw: &str) -> BaseItemKind {
    match raw {
        "Movie" => BaseItemKind::Movie,
        "Series" => BaseItemKind::Series,
        "Season" => BaseItemKind::Season,
        "Episode" => BaseItemKind::Episode,
        "BoxSet" => BaseItemKind::BoxSet,
        "Video" => BaseItemKind::Video,
        "Audio" => BaseItemKind::Audio,
        "MusicAlbum" => BaseItemKind::MusicAlbum,
        "MusicArtist" => BaseItemKind::MusicArtist,
        "Playlist" => BaseItemKind::Playlist,
        "Person" => BaseItemKind::Person,
        "Genre" => BaseItemKind::Genre,
        "Studio" => BaseItemKind::Studio,
        "PhotoAlbum" => BaseItemKind::PhotoAlbum,
        "UserView" => BaseItemKind::UserView,
        _ => BaseItemKind::Folder,
    }
}

fn media_name(m: &MediaType) -> &'static str {
    match m {
        MediaType::Video => "Video",
        MediaType::Audio => "Audio",
        MediaType::Photo => "Photo",
        MediaType::Book => "Book",
    }
}

pub fn parse_media_type(raw: &str) -> Option<MediaType> {
    Some(match raw {
        "Video" => MediaType::Video,
        "Audio" => MediaType::Audio,
        "Photo" => MediaType::Photo,
        "Book" => MediaType::Book,
        _ => return None,
    })
}

fn parse_collection_type(raw: &str) -> Option<CollectionType> {
    Some(match raw.to_ascii_lowercase().as_str() {
        "movies" => CollectionType::Movies,
        "tvshows" => CollectionType::TvShows,
        "music" => CollectionType::Music,
        "musicvideos" => CollectionType::MusicVideos,
        "books" => CollectionType::Books,
        "photos" => CollectionType::Photos,
        "playlists" => CollectionType::Playlists,
        _ => return None,
    })
}

/// Canonical URL for an external provider id.
fn provider_url(provider: &str, value: &str) -> Option<String> {
    let lower = provider.to_ascii_lowercase();
    Some(match lower.as_str() {
        "imdb" => format!("https://www.imdb.com/title/{}/", value.trim_start_matches("tt")),
        "tmdb" => format!("https://www.themoviedb.org/{}", {
            // TMDB ids are ambiguous between movie and tv without the context;
            // linking to search is honest about that.
            format!("search?query={value}")
        }),
        "tvdb" => format!("https://thetvdb.com/?tab=series&id={value}"),
        "person" => format!("https://www.themoviedb.org/person/{value}"),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sort_keys_map_to_sql() {
        assert_eq!(SortBy::parse("SortName").unwrap().sql(), "i.sort_name COLLATE NOCASE");
        assert_eq!(SortBy::parse("ProductionYear").unwrap().sql(), "i.production_year");
        assert!(SortBy::parse("'; DROP TABLE items; --").is_none());
        assert!(SortBy::parse("NotAField").is_none());
    }

    #[test]
    fn like_wildcards_are_escaped() {
        assert_eq!(escape_like("100%"), "100\\%");
        assert_eq!(escape_like("a_b"), "a\\_b");
        assert_eq!(escape_like("plain"), "plain");
    }

    #[test]
    fn placeholders_render() {
        assert_eq!(placeholders(0), "");
        assert_eq!(placeholders(1), "?");
        assert_eq!(placeholders(3), "?, ?, ?");
    }

    #[test]
    fn kind_round_trip() {
        for kind in [BaseItemKind::Movie, BaseItemKind::Series, BaseItemKind::Episode] {
            assert_eq!(parse_kind(kind_name(&kind)), kind);
        }
    }

    #[test]
    fn field_selection_parses() {
        let f = FieldSelection::parse(Some("Overview,Genres,MediaSources"));
        assert!(f.overview && f.genres && f.media_sources);
        assert!(!f.people);

        assert!(!FieldSelection::parse(None).overview);
        assert!(FieldSelection::all().overview);
    }

    #[test]
    fn provider_urls() {
        assert!(provider_url("imdb", "tt0133093").unwrap().contains("0133093"));
        assert!(provider_url("unknown", "x").is_none());
    }
}