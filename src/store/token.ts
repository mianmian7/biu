import { create } from "zustand";
import { persist } from "zustand/middleware";

import { nowUnix } from "@/common/utils/time";

interface TokenState {
  /** 刷新 cookie 使用 */
  tokenData?: {
    refresh_token?: string;
  };
  /** 下次检测刷新时间 */
  nextCheckRefreshTime?: number;
}

interface Action {
  updateToken: (info: Partial<TokenState>) => void;
  clear: () => void;
}

export const useToken = create<TokenState & Action>()(
  persist(
    set => ({
      tokenData: {},
      nextCheckRefreshTime: nowUnix(),
      updateToken: async (info: Partial<TokenState>) => {
        set(state => ({ ...state, ...info }));
      },
      clear: () => {
        set({
          tokenData: undefined,
          nextCheckRefreshTime: undefined,
        });
      },
    }),
    {
      name: "user-token",
      partialize: state => ({
        tokenData: state.tokenData,
        nextCheckRefreshTime: state.nextCheckRefreshTime,
      }),
    },
  ),
);
