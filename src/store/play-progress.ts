import { create } from "zustand";

interface ProgressState {
  currentTime: number;
  setCurrentTime: (time: number) => void;
  initCurrentTime: (playId?: string) => number;
  saveCurrentTime: (playId?: string) => void;
}

export const usePlayProgress = create<ProgressState>((set, get) => ({
  currentTime: 0,
  initCurrentTime: playId => {
    let currentTime = 0;
    try {
      const saved = JSON.parse(localStorage.getItem("play-current-time") || "null") as {
        playId?: unknown;
        currentTime?: unknown;
      } | null;
      if (
        playId &&
        saved?.playId === playId &&
        typeof saved.currentTime === "number" &&
        Number.isFinite(saved.currentTime) &&
        saved.currentTime >= 0
      ) {
        currentTime = saved.currentTime;
      } else {
        localStorage.removeItem("play-current-time");
      }
    } catch {
      localStorage.removeItem("play-current-time");
    }
    set({ currentTime });
    return currentTime;
  },
  saveCurrentTime: playId => {
    const { currentTime } = get();
    if (playId && Number.isFinite(currentTime) && currentTime >= 0) {
      localStorage.setItem("play-current-time", JSON.stringify({ playId, currentTime }));
    } else {
      localStorage.removeItem("play-current-time");
    }
  },
  setCurrentTime: (time: number) => set({ currentTime: time }),
}));
