export const CREATE_TYPES = [
  "goal",
  "plan",
  "stage",
  "requirement",
  "issue",
  "work",
  "role",
  "resource",
] as const;

export const PLAN_STATUSES = [
  "draft",
  "active",
  "paused",
  "completed",
  "cancelled",
] as const;
export const STAGE_STATUSES = [
  "planned",
  "active",
  "paused",
  "completed",
  "cancelled",
] as const;
export const REQUIREMENT_STATUSES = [
  "proposed",
  "ready",
  "in_progress",
  "satisfied",
  "withdrawn",
] as const;
export const ISSUE_STATUSES = [
  "open",
  "in_progress",
  "resolved",
  "closed",
] as const;
export const WORK_STATUSES = [
  "pending",
  "in_progress",
  "paused",
  "submitted",
  "completed",
  "cancelled",
] as const;
export const PRIORITIES = ["low", "normal", "high", "urgent"] as const;
export const RESOURCE_TYPES = [
  "repository",
  "document",
  "design",
  "service",
  "environment",
  "artifact",
  "url",
] as const;
export const LOCATOR_TYPES = [
  "url",
  "nostr_address",
  "nostr_event",
  "buzz_deep_link",
] as const;
