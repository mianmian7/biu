import Store from "electron-store";

import type { ReferenceCandidate } from "../../shared/auto-lyrics/reference";

import { lyricsCacheStore } from "../store";
import { getUserDataPath } from "../utils";
import { decodeAudio } from "./decode";
import { AutoLyricsService } from "./service";
import { sources } from "./sources";
import { alignAudio } from "./worker-client";

const referencesStore = new Store<Record<string, ReferenceCandidate>>({
  name: "auto-lyrics-references",
  cwd: getUserDataPath(),
});

export const autoLyricsService = new AutoLyricsService({
  sources,
  decode: decodeAudio,
  align: alignAudio,
  cache: {
    get: key => lyricsCacheStore.get(key),
    set: (key, value) => lyricsCacheStore.set(key, value),
  },
  references: {
    get: id => referencesStore.get(String(id)),
    set: (id, value) => referencesStore.set(String(id), value),
  },
});
