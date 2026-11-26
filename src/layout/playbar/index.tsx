import { Card } from "@heroui/react";

import { usePlayList } from "@/store/play-list";

import Center from "./center";
import Left from "./left";
import Right from "./right";

/**
 * 播放任务栏
 */
function PlayBar() {
  const playId = usePlayList(s => s.playId);
  // 播放器初始化在 App 中只执行一次：窗口隐藏后 PlayBar 会被卸载，重新挂载时不能再次 init（会重新加载音频）
  return (
    <Card
      radius="none"
      shadow="sm"
      className="bg-background grid h-full grid-cols-[minmax(0,1fr)_minmax(0,3fr)_minmax(0,1fr)] px-4"
    >
      <div className="h-full">{Boolean(playId) && <Left />}</div>
      <Center />
      <Right />
    </Card>
  );
}

export default PlayBar;
