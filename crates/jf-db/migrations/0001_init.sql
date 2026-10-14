-- Initial schema for a multi-user media server.
--
-- Item and user shapes follow Jellyfin conventions so third-party clients and
-- the C# reference implementation stay usable as a spec.

CREATE TABLE server_info (
    id                  TEXT PRIMARY KEY NOT NULL,
    name                TEXT NOT NULL,
    version             TEXT NOT NULL,
    startup_completed   INTEGER NOT NULL DEFAULT 0
) STRICT;

CREATE TABLE users (
    id                  TEXT PRIMARY KEY NOT NULL,
    username            TEXT NOT NULL,
    username_lower      TEXT NOT NULL UNIQUE,
    password_hash       TEXT,
    server_id           TEXT NOT NULL,
    is_hidden           INTEGER NOT NULL DEFAULT 0,
    is_disabled         INTEGER NOT NULL DEFAULT 0,
    last_login_date     TEXT,
    last_activity_date  TEXT,
    created_at          TEXT NOT NULL,
    policy_json         TEXT NOT NULL DEFAULT '{}',
    config_json         TEXT NOT NULL DEFAULT '{}',
    avatar_image_tag    TEXT
) STRICT;

-- A library is a collection folder that owns a tree of items.
CREATE TABLE libraries (
    id                  TEXT PRIMARY KEY NOT NULL,
    name                TEXT NOT NULL,
    path                TEXT NOT NULL UNIQUE,
    collection_type     TEXT NOT NULL,
    created_at          TEXT NOT NULL,
    enabled             INTEGER NOT NULL DEFAULT 1
) STRICT;

CREATE TABLE items (
    id                  TEXT PRIMARY KEY NOT NULL,
    library_id          TEXT NOT NULL REFERENCES libraries(id) ON DELETE CASCADE,
    kind                TEXT NOT NULL,
    media_type          TEXT,
    name                TEXT NOT NULL,
    sort_name           TEXT,
    path                TEXT NOT NULL,
    size                INTEGER,
    container           TEXT,
    date_created        TEXT,
    date_modified       TEXT,
    production_year     INTEGER,
    index_number        INTEGER,
    parent_index_number INTEGER,
    overview            TEXT,
    official_rating     TEXT,
    community_rating    REAL,
    critic_rating       REAL,
    status              TEXT,
    premiere_date       TEXT,
    run_time_ticks      INTEGER,
    parent_id           TEXT REFERENCES items(id) ON DELETE CASCADE,
    series_id           TEXT,
    series_name         TEXT,
    season_id           TEXT,
    season_name         TEXT,
    episode_title       TEXT,
    album               TEXT,
    album_id            TEXT,
    album_artist        TEXT,
    genres_json         TEXT NOT NULL DEFAULT '[]',
    tags_json           TEXT NOT NULL DEFAULT '[]',
    artists_json        TEXT NOT NULL DEFAULT '[]',
    taglines_json       TEXT NOT NULL DEFAULT '[]',
    people_json         TEXT NOT NULL DEFAULT '[]',
    media_sources_json  TEXT NOT NULL DEFAULT '[]',
    image_tags_json     TEXT NOT NULL DEFAULT '{}',
    backdrop_json       TEXT NOT NULL DEFAULT '[]',
    is_folder           INTEGER NOT NULL DEFAULT 0,
    date_last_refreshed TEXT
) STRICT;

CREATE INDEX idx_items_library ON items(library_id);
CREATE INDEX idx_items_parent ON items(parent_id);
CREATE INDEX idx_items_kind ON items(kind);
CREATE INDEX idx_items_series ON items(series_id);
CREATE INDEX idx_items_name ON items(name);

-- Provider ids (tmdb, omdb, imdb, tvdb...) resolved by metadata plugins.
CREATE TABLE item_external_ids (
    item_id             TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    provider            TEXT NOT NULL,
    provider_key        TEXT NOT NULL,
    value               TEXT NOT NULL,
    PRIMARY KEY (item_id, provider, provider_key)
) STRICT;

CREATE INDEX idx_external_lookup
    ON item_external_ids(provider, provider_key, value);

CREATE TABLE images (
    item_id             TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    image_type          TEXT NOT NULL,
    index_num           INTEGER NOT NULL DEFAULT 0,
    path                TEXT NOT NULL,
    width               INTEGER,
    height              INTEGER,
    size                INTEGER,
    tag                 TEXT NOT NULL,
    PRIMARY KEY (item_id, image_type, index_num)
) STRICT;

CREATE TABLE media_streams (
    id                  TEXT PRIMARY KEY NOT NULL,
    item_id             TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    stream_index        INTEGER NOT NULL,
    stream_type         TEXT NOT NULL,
    codec               TEXT,
    language            TEXT,
    title               TEXT,
    width               INTEGER,
    height              INTEGER,
    channels            INTEGER,
    channel_layout      TEXT,
    sample_rate         INTEGER,
    bitrate             INTEGER,
    is_default          INTEGER NOT NULL DEFAULT 0,
    is_forced           INTEGER NOT NULL DEFAULT 0,
    is_external         INTEGER NOT NULL DEFAULT 0,
    path                TEXT
) STRICT;

CREATE INDEX idx_streams_item ON media_streams(item_id);

-- Per-user playback state. This is the heart of per-user profiles.
CREATE TABLE user_data (
    user_id             TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    item_id             TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    playback_position_ticks INTEGER NOT NULL DEFAULT 0,
    play_count          INTEGER NOT NULL DEFAULT 0,
    is_favorite         INTEGER NOT NULL DEFAULT 0,
    played              INTEGER NOT NULL DEFAULT 0,
    last_played_date    TEXT,
    audio_stream_index  INTEGER,
    subtitle_stream_index INTEGER,
    PRIMARY KEY (user_id, item_id)
) STRICT;

CREATE INDEX idx_userdata_item ON user_data(item_id);

-- Access-token sessions, one row per logged-in client.
CREATE TABLE sessions (
    id                  TEXT PRIMARY KEY NOT NULL,
    user_id             TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    access_token        TEXT NOT NULL UNIQUE,
    client              TEXT NOT NULL,
    device_name         TEXT NOT NULL,
    device_id           TEXT NOT NULL,
    application_version TEXT NOT NULL,
    created_at          TEXT NOT NULL,
    last_activity_date  TEXT
) STRICT;

CREATE INDEX idx_sessions_user ON sessions(user_id);

-- Live ffmpeg transcode processes, reaped on startup.
CREATE TABLE transcode_sessions (
    id                  TEXT PRIMARY KEY NOT NULL,
    user_id             TEXT NOT NULL,
    item_id             TEXT NOT NULL,
    play_session_id     TEXT NOT NULL UNIQUE,
    protocol            TEXT NOT NULL,
    container           TEXT,
    audio_codec         TEXT,
    video_codec         TEXT,
    subtitle_method     TEXT,
    audio_stream_index  INTEGER,
    subtitle_stream_index INTEGER,
    transcode_hls       INTEGER NOT NULL DEFAULT 0,
    transcode_audio     INTEGER NOT NULL DEFAULT 0,
    transcode_video     INTEGER NOT NULL DEFAULT 0,
    direct_stream       INTEGER NOT NULL DEFAULT 0,
    direct_play         INTEGER NOT NULL DEFAULT 0,
    allow_video_stream_copy INTEGER NOT NULL DEFAULT 1,
    allow_audio_stream_copy INTEGER NOT NULL DEFAULT 1,
    runtime_ticks       INTEGER,
    pid                 INTEGER,
    started_at          TEXT NOT NULL,
    last_activity_date  TEXT
) STRICT;

-- Persisted plugin settings, namespaced per plugin.
CREATE TABLE plugin_config (
    plugin_id           TEXT NOT NULL,
    key                 TEXT NOT NULL,
    value               TEXT NOT NULL,
    PRIMARY KEY (plugin_id, key)
) STRICT;

-- Playback progress for a transcode, used to resume HLS sessions.
CREATE TABLE transcode_playstate (
    play_session_id     TEXT PRIMARY KEY NOT NULL,
    position_ticks      INTEGER NOT NULL DEFAULT 0,
    updated_at          TEXT NOT NULL
) STRICT;