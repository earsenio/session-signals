import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { DEFAULT_CONFIG, type Config } from "./config";

/// One initial fetch per window (module-level), shared by every hook instance —
/// `useTheme` and the widget both subscribe, and neither needs its own IPC
/// round-trip. Freshness after that comes from `config-updated` events, so the
/// cached promise never goes stale while the window lives.
let initialFetch: Promise<Config> | null = null;

/// Subscribe to the backend config: the persisted snapshot on mount, then every
/// `config-updated` broadcast — so a change made in one window (e.g. the port,
/// edited in Settings) is reflected everywhere (e.g. the widget footer) without
/// a reload. Read-only; writes go through the `set_config` command.
export function useConfig(): Config {
  const [config, setConfig] = useState<Config>(DEFAULT_CONFIG);

  useEffect(() => {
    let active = true;
    initialFetch ??= invoke<Config>("get_config");
    initialFetch
      .then((c) => {
        if (active) setConfig(c);
      })
      .catch(() => {});
    const un = listen<Config>("config-updated", (e) => {
      if (active) setConfig(e.payload);
    });
    return () => {
      active = false;
      void un.then((u) => u());
    };
  }, []);

  return config;
}
