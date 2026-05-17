import { RATING_NAMES, DIFFICULTY_NAMES, LENGTH_NAMES } from './types';
import type { SubmissionNotesObject } from './types';

export function unwrap<T>(payload: unknown): T {
  if (payload && typeof payload === 'object' && 'data' in payload) {
    return (payload as { data: T }).data;
  }
  return payload as T;
}

export async function fetchJson<T = unknown>(url: string, init?: RequestInit): Promise<T> {
  const response = await fetch(url, init);
  const text = await response.text();
  const payload = text ? JSON.parse(text) : null;

  if (!response.ok) {
    const message = payload && typeof payload === 'object'
      ? (payload.message || payload.error)
      : null;
    throw new Error(message || `Request failed (${response.status})`);
  }

  return payload as T;
}

export function formatMonth(period: string) {
  const date = new Date(`${period}T00:00:00`);
  return Number.isNaN(date.getTime())
    ? period.slice(0, 7)
    : date.toLocaleString(undefined, { month: 'short', year: '2-digit' });
}

export function formatHour(timestamp: string) {
  const date = new Date(timestamp);
  return Number.isNaN(date.getTime())
    ? timestamp.slice(11, 16)
    : date.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
}

export function formatDateTime(timestamp: string) {
  const date = new Date(timestamp);
  if (Number.isNaN(date.getTime())) return timestamp;
  return date.toLocaleString(undefined, {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  });
}

export function parseSubmissionNote(note: string | null): SubmissionNotesObject | null {
  if (!note) return null;

  const parts = note.split(';');
  const data: Record<string, string> = {};
  for (const part of parts) {
    const [key, ...rest] = part.split('=');
    if (key && rest.length > 0 && data[key] === undefined) {
      data[key] = rest.join('=');
    }
  }

  if (!data.v || data.v !== '1') return null;

  return {
    level_name: data.ln ? decodeURIComponent(data.ln) : null,
    creator_id: data.ci ? Number(data.ci) : null,
    creator_name: data.cn || null,
    downloads: data.dw ? Number(data.dw) : null,
    likes: data.lk ? Number(data.lk) : null,
    stars: data.ls ? Number(data.ls) : null,
    length: LENGTH_NAMES[Number(data.ll)] || null,
    rating: RATING_NAMES[Number(data.lr)] || 'NA',
    difficulty: DIFFICULTY_NAMES[Number(data.ld)] || 'NA',
    percentage: data.pr ? Number(data.pr) : null,
    attempt_time: data.tm ? Number(data.tm) : null,
    message: data.m ? decodeURIComponent(data.m) : null,
  };
}