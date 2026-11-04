import { invokeTauri } from "@/platform";

function formatArgs(args: unknown[]): string {
  return args
    .map(arg => {
      if (arg instanceof Error) {
        return arg.stack || arg.message;
      }
      if (typeof arg === "string") {
        return arg;
      }
      try {
        return JSON.stringify(arg);
      } catch {
        return String(arg);
      }
    })
    .join(" ");
}

function sendLog(level: "info" | "warn" | "error", args: unknown[]): void {
  const message = formatArgs(args);
  void invokeTauri("frontend_log", { level, message }).catch(() => {});
}

const log = {
  info: (...args: unknown[]) => {
    console.info(...args);
    sendLog("info", args);
  },
  warn: (...args: unknown[]) => {
    console.warn(...args);
    sendLog("warn", args);
  },
  error: (...args: unknown[]) => {
    console.error(...args);
    sendLog("error", args);
  },
  debug: (...args: unknown[]) => {
    console.debug(...args);
  },
  verbose: (...args: unknown[]) => {
    console.debug(...args);
  },
  log: (...args: unknown[]) => {
    console.log(...args);
    sendLog("info", args);
  },
};

export default log;
