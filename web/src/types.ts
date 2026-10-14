/**
 * Wire shapes for the Jellyfin-compatible API.
 *
 * These mirror the server's DTOs. Only the fields the client actually reads are
 * modelled; the server sends more, and extra properties are ignored at runtime.
 */

export type MediaType = "Video" | "Audio" | "Photo" | "Book";

export type ItemType =
  | "Movie"
  | "Series"
  | "Season"
  | "Episode"
  | "MusicAlbum"
  | "Audio"
  | "BoxSet"
  | "Photo"
  | "Folder"
  | "UserView"
  | "CollectionFolder";

export interface UserPolicy {
  IsAdministrator: boolean;
  EnableMediaPlayback: boolean;
  EnableAllFolders: boolean;
  EnabledFolders: string[];
}

export interface User {
  Id: string;
  Name: string;
  ServerId: string;
  PrimaryImageTag: string | null;
  HasPassword: boolean;
  Policy: UserPolicy;
}

export interface UserData {
  PlaybackPositionTicks: number;
  PlayCount: number;
  IsFavorite: boolean;
  Played: boolean;
  UnplayedItemCount: number;
}

export interface BaseItem {
  Id: string;
  Name: string;
  Type: ItemType;
  ServerId: string;
  IsFolder: boolean;
  MediaType?: MediaType;
  CollectionType?: string | null;
  Path?: string;
  ProductionYear?: number | null;
  RunTimeTicks?: number | null;
  Overview?: string | null;
  Genres?: string[];
  OfficialRating?: string | null;
  CommunityRating?: number | null;
  CriticRating?: number | null;
  IndexNumber?: number | null;
  ParentIndexNumber?: number | null;
  ParentId?: string | null;
  SeriesName?: string | null;
  SeriesId?: string | null;
  SeasonId?: string | null;
  EpisodeTitle?: string | null;
  Taglines?: string[];
  ImageTags?: Record<string, string> | null;
  BackdropImageTags?: string[];
  UserData?: UserData;
}

export interface QueryResult<T> {
  Items: T[];
  TotalRecordCount: number;
  StartIndex: number;
}

export interface PublicSystemInfo {
  ServerName: string;
  Version: string;
  Id: string;
  StartupWizardCompleted: boolean;
}

export interface PublicUser {
  Id: string;
  Name: string;
  ServerId: string;
  PrimaryImageTag: string | null;
  HasPassword: boolean;
  Policy: UserPolicy;
}

export interface PublicUsers {
  ServerId: string;
  ServerName: string;
  Users: PublicUser[];
}

export interface AuthenticationResult {
  User: User;
  AccessToken: string;
  ServerId: string;
}

export interface MediaSource {
  Id: string;
  Protocol: string;
  Path: string;
  Container: string | null;
  Size: number | null;
  SupportsDirectPlay: boolean;
  SupportsTranscoding: boolean;
  MediaStreams: Array<{
    Codec: string | null;
    Type: string;
    Index: number;
    Width?: number | null;
    Height?: number | null;
    Language?: string | null;
    IsDefault: boolean;
  }>;
}

export interface PlaybackInfo {
  MediaSources: MediaSource[];
  PlaySessionId: string;
}