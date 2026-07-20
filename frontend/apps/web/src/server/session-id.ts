export const SESSION_ID_PATTERN = /^sess_[A-Za-z0-9_-]{32}$/;

export function validSessionId(id: string): boolean {
  return SESSION_ID_PATTERN.test(id);
}
