import { type InternalAxiosRequestConfig } from "axios";

import { refreshCookie } from "@/common/utils/cookie";
import { addSecondsUnix, nowUnix } from "@/common/utils/time";
import { useToken } from "@/store/token";

import { encodeParamsWbi } from "./wbi-sign";

let refreshCookiePromise: Promise<any> | null = null;

export const requestInterceptors = async (config: InternalAxiosRequestConfig) => {
  if (!config.skipRefreshCheck && (useToken.getState().nextCheckRefreshTime || 0) < nowUnix()) {
    if (!refreshCookiePromise) {
      useToken.setState({ nextCheckRefreshTime: addSecondsUnix(30) });
      refreshCookiePromise = refreshCookie().finally(() => {
        refreshCookiePromise = null;
      });
    }
    try {
      await refreshCookiePromise;
    } finally {
      useToken.setState({ nextCheckRefreshTime: addSecondsUnix(2 * 24 * 60 * 60) });
    }
  }

  if (config.useCSRF) {
    const csrfToken = await window.electron.getCookie("bili_jct");
    if (csrfToken) {
      if (config.method === "post") {
        config.data ??= {};
        config.data.csrf = csrfToken;
      } else {
        config.params ??= {};
        config.params.csrf = csrfToken;
      }
    }
  }

  if (config.useWbi) {
    config.params ??= {};
    config.params = await encodeParamsWbi(config.params);
  }

  if (config.useFormData) {
    const formData = new FormData();
    for (const key in config.data) {
      formData.append(key, config.data[key]);
    }
    config.data = formData;
  }

  return config;
};
