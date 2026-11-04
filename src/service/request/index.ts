import axios, {
  AxiosError,
  AxiosHeaders,
  type AxiosAdapter,
  type AxiosResponse,
  type CreateAxiosDefaults,
  type InternalAxiosRequestConfig,
} from "axios";

import { invokeTauri } from "@/platform";

import { requestInterceptors } from "./request-interceptors";
import { geetestInterceptors } from "./response-interceptors";

interface BiliRequestPayload {
  method: string;
  url: string;
  params?: Record<string, unknown>;
  body?: string;
  headers?: Record<string, string>;
}

interface BiliResponsePayload {
  status: number;
  headers: Record<string, string>;
  body: string;
}

function serializeRequestBody(
  config: InternalAxiosRequestConfig,
  headers: Record<string, string>,
): string | undefined {
  const data = config.data;
  if (data === undefined || data === null) {
    return undefined;
  }
  if (typeof FormData !== "undefined" && data instanceof FormData) {
    const params = new URLSearchParams();
    data.forEach((value, key) => {
      if (typeof value === "string") {
        params.append(key, value);
      }
    });
    if (!Object.keys(headers).some(k => k.toLowerCase() === "content-type")) {
      headers["Content-Type"] = "application/x-www-form-urlencoded";
    }
    return params.toString();
  }
  if (typeof URLSearchParams !== "undefined" && data instanceof URLSearchParams) {
    if (!Object.keys(headers).some(k => k.toLowerCase() === "content-type")) {
      headers["Content-Type"] = "application/x-www-form-urlencoded";
    }
    return data.toString();
  }
  if (typeof data === "string") {
    return data;
  }
  const contentTypeKey = Object.keys(headers).find(k => k.toLowerCase() === "content-type");
  const contentType = contentTypeKey ? headers[contentTypeKey].toLowerCase() : "";
  if (contentType.includes("application/x-www-form-urlencoded") && typeof data === "object") {
    const params = new URLSearchParams();
    for (const [k, v] of Object.entries(data as Record<string, unknown>)) {
      if (v !== undefined && v !== null) {
        params.append(k, String(v));
      }
    }
    return params.toString();
  }
  if (!contentTypeKey) {
    headers["Content-Type"] = "application/json";
  }
  return JSON.stringify(data);
}

export const biliAdapter: AxiosAdapter = async (config: InternalAxiosRequestConfig) => {
  const baseURL = config.baseURL || "";
  const rawUrl = config.url || "";
  const resolvedUrl =
    rawUrl.startsWith("http://") || rawUrl.startsWith("https://")
      ? rawUrl
      : `${baseURL.replace(/\/+$/, "")}/${rawUrl.replace(/^\/+/, "")}`;

  const headersObj: Record<string, string> = {};
  if (config.headers) {
    const rawHeaders =
      typeof (config.headers as AxiosHeaders).toJSON === "function"
        ? ((config.headers as AxiosHeaders).toJSON() as Record<string, unknown>)
        : (config.headers as Record<string, unknown>);
    for (const [k, v] of Object.entries(rawHeaders)) {
      if (v !== undefined && v !== null && typeof v !== "object") {
        headersObj[k] = String(v);
      }
    }
  }

  const body = serializeRequestBody(config, headersObj);
  const requestPayload: BiliRequestPayload = {
    method: (config.method || "get").toUpperCase(),
    url: resolvedUrl,
    params:
      config.params && typeof config.params === "object"
        ? (config.params as Record<string, unknown>)
        : undefined,
    body,
    headers: headersObj,
  };

  const res = await invokeTauri<BiliResponsePayload>("bili_request", {
    request: requestPayload,
  });

  let parsedData: unknown = res.body;
  if (config.responseType !== "text") {
    try {
      parsedData = JSON.parse(res.body);
    } catch {
      parsedData = res.body;
    }
  }

  const response: AxiosResponse = {
    data: parsedData,
    status: res.status,
    statusText: String(res.status),
    headers: res.headers || {},
    config,
  };

  const validateStatus = config.validateStatus ?? (status => status >= 200 && status < 300);
  if (!validateStatus(res.status)) {
    throw new AxiosError(
      `Request failed with status code ${res.status}`,
      res.status >= 500 ? AxiosError.ERR_BAD_RESPONSE : AxiosError.ERR_BAD_REQUEST,
      config,
      requestPayload,
      response,
    );
  }

  return response;
};

const axiosConfig: CreateAxiosDefaults = {
  timeout: 10000,
  withCredentials: true,
  adapter: biliAdapter,
};

export const axiosInstance = axios.create(axiosConfig);

export const searchRequest = axios.create({
  ...axiosConfig,
  baseURL: "https://s.search.bilibili.com",
});

export const biliRequest = axios.create({
  ...axiosConfig,
  baseURL: "https://www.bilibili.com",
});

export const memberRequest = axios.create({
  ...axiosConfig,
  baseURL: "https://member.bilibili.com",
});

export const apiRequest = axios.create({
  ...axiosConfig,
  baseURL: "https://api.bilibili.com",
});

export const passportRequest = axios.create({
  ...axiosConfig,
  baseURL: "https://passport.bilibili.com",
});

apiRequest.interceptors.request.use(requestInterceptors);
passportRequest.interceptors.request.use(requestInterceptors);
searchRequest.interceptors.request.use(requestInterceptors);
memberRequest.interceptors.request.use(requestInterceptors);

apiRequest.interceptors.response.use(geetestInterceptors);

axiosInstance.interceptors.response.use(res => res.data);
biliRequest.interceptors.response.use(res => res.data);
apiRequest.interceptors.response.use(res => res.data);
passportRequest.interceptors.response.use(res => res.data);
searchRequest.interceptors.response.use(res => res.data);
memberRequest.interceptors.response.use(res => res.data);
