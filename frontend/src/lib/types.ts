export interface UserStats {
  id: number;
  account_id: number;
  username: string;
  role: 'user' | 'verified' | 'moderator' | 'admin' | 'owner';
  upload_count: number;
  accepted_upload_count: number;
  pending_upload_count: number;
  level_count: number;
  accepted_level_count: number;
  active_thumbnail_count: number;
}

export interface StatsResponse {
  storage: number;
  thumbnails: number;
  users_per_month: number;
  users_total: number;
  uploads_total: number;
  pending_uploads_total: number;
  accepted_uploads_total: number;
  total_levels: number;
  current_pending_uploads: number;
}

export interface UserHistoryPoint {
  period: string;
  upload_count: number;
  accepted_upload_count: number;
  pending_upload_count: number;
  level_count: number;
  accepted_level_count: number;
}

export interface StatsHistoryPoint {
  captured_at: string;
  storage_bytes: number;
  thumbnails_count: number;
  users_per_month: number | null;
  users_total: number;
  uploads_total: number;
  pending_uploads_total: number;
  accepted_uploads_total: number;
}

export interface ServerSettings {
  pause_submissions: boolean;
  min_supported_client: string;
}

export const RATING_NAMES = ['NA', 'Rated', 'Featured', 'Epic', 'Legendary', 'Mythic'] as const;
export const DIFFICULTY_NAMES = ['NA', 'Auto', 'Easy', 'Normal', 'Hard', 'Harder', 'Insane', 'Easy Demon', 'Medium Demon', 'Hard Demon', 'Insane Demon', 'Extreme Demon'] as const;
export const LENGTH_NAMES = ['Tiny', 'Short', 'Medium', 'Long', 'XL', 'Plat'] as const;

export interface SubmissionNotesObject {
  level_name: string | null;
  creator_id: number | null;
  creator_name: string | null;
  downloads: number | null;
  likes: number | null;
  stars: number | null;
  length: typeof LENGTH_NAMES[number] | null;
  rating: typeof RATING_NAMES[number] | null;
  difficulty: typeof DIFFICULTY_NAMES[number] | null;
  percentage: number | null;
  attempt_time: number | null;
  message: string | null;
}

export interface PendingItem {
  id: number;
  user_id: number;
  username: string;
  level_id: number;
  accepted: boolean;
  replacement: boolean;
  upload_time: string;
  submission_note: string | null;
  account_id: number | null;
  user_role: 'user' | 'verified' | 'moderator' | 'admin' | 'owner';
  note_data: SubmissionNotesObject | null | undefined; // populated on load
}

export interface PendingResponse {
  uploads: PendingItem[];
  page: number;
  per_page: number;
  total: number;
}

export interface UserRow {
  id: number;
  username: string;
  account_id: number;
  discord_id: string | null;
  role: 'user' | 'verified' | 'moderator' | 'admin' | 'owner';
  total_uploads: number;
  accepted: number;
  pending: number;
  rejected: number;
  active_thumbnails: number;
  banned: boolean;
}

export interface UserListResponse {
  users: UserRow[];
  page: number;
  per_page: number;
  total: number;
  total_pages: number;
}