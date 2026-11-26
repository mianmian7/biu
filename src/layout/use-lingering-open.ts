import { useEffect, useState } from "react";

/** 弹层关闭后保持挂载的时长，让 HeroUI 的退出动画播完再卸载 */
export const OVERLAY_EXIT_LINGER_MS = 500;

/** 打开时立即为 true；关闭后延迟 `lingerMs` 再变 false（期间重新打开则保持挂载） */
export function useLingeringOpen(isOpen: boolean, lingerMs = OVERLAY_EXIT_LINGER_MS) {
  const [mounted, setMounted] = useState(isOpen);
  if (isOpen && !mounted) setMounted(true);

  useEffect(() => {
    if (isOpen || !mounted) return;
    const timer = setTimeout(() => setMounted(false), lingerMs);
    return () => clearTimeout(timer);
  }, [isOpen, mounted, lingerMs]);

  return isOpen || mounted;
}
