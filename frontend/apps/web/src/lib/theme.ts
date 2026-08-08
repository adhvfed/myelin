export const THEMES = ["dark", "light", "high-contrast"] as const;
export type Theme = typeof THEMES[number];
export const THEME_STORAGE_KEY = "myelin.appearance";

export function isTheme(value: unknown): value is Theme {
  return typeof value === "string" && THEMES.includes(value as Theme);
}

export function nextTheme(current: Theme): Theme {
  return THEMES[(THEMES.indexOf(current) + 1) % THEMES.length] ?? "dark";
}

export function applyTheme(theme: Theme, root = document.documentElement, storage: Pick<Storage, "setItem"> | null = localStorage): Theme {
  root.dataset.theme = theme;
  storage?.setItem(THEME_STORAGE_KEY, theme);
  return theme;
}

export function restoreTheme(root = document.documentElement, storage: Pick<Storage, "getItem"> | null = localStorage): Theme {
  const stored = storage?.getItem(THEME_STORAGE_KEY);
  const theme = isTheme(stored) ? stored : isTheme(root.dataset.theme) ? root.dataset.theme : "dark";
  root.dataset.theme = theme;
  return theme;
}

export function cycleTheme(root = document.documentElement, storage: Storage | null = localStorage): Theme {
  const current = isTheme(root.dataset.theme) ? root.dataset.theme : "dark";
  return applyTheme(nextTheme(current), root, storage);
}
