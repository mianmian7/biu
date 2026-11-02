import React, { useCallback, useMemo } from "react";

import { Button, Radio, RadioGroup, Slider, Switch } from "@heroui/react";
import { RiCheckLine } from "@remixicon/react";
import clsx from "classnames";
import { useShallow } from "zustand/react/shallow";

import ColorPicker from "@/components/color-picker";
import DesktopLyricsView from "@/components/desktop-lyrics-view";
import FontSelect from "@/components/font-select";
import { useSettings } from "@/store/settings";
import { defaultAppSettings } from "@shared/settings/app-settings";
import {
  DESKTOP_LYRICS_FONT_SIZE_MAX,
  DESKTOP_LYRICS_FONT_SIZE_MIN,
  DESKTOP_LYRICS_OPACITY_MIN,
  DESKTOP_LYRICS_SUB_SCALE_MAX,
  DESKTOP_LYRICS_SUB_SCALE_MIN,
  defaultDesktopLyricsStyle,
  desktopLyricsColorPresets,
  resolveDesktopLyricsColors,
  resolveDesktopLyricsStyle,
} from "@shared/settings/desktop-lyrics";

/** 预览进度循环周期 */
const PREVIEW_CYCLE_MS = 4000;

const SettingRow = ({
  title,
  description,
  children,
}: {
  title: string;
  description?: string;
  children: React.ReactNode;
}) => (
  <div className="flex w-full items-center justify-between">
    <div className="mr-6 space-y-1">
      <div className="text-medium font-medium">{title}</div>
      {description ? <div className="text-sm text-zinc-500">{description}</div> : null}
    </div>
    {children}
  </div>
);

const Swatch = ({
  title,
  colors,
  selected,
  onPress,
}: {
  title: string;
  colors: DesktopLyricsColors;
  selected: boolean;
  onPress?: () => void;
}) => (
  <button
    type="button"
    title={title}
    onClick={onPress}
    className={clsx(
      "flex cursor-pointer flex-col items-center gap-1 rounded-lg p-1 text-xs transition-colors",
      selected ? "bg-primary/15 text-primary" : "text-foreground-500 hover:bg-default-100",
    )}
  >
    <span
      className="relative flex h-8 w-12 items-center justify-center overflow-hidden rounded-md border border-black/10 text-base font-black"
      style={{ background: "linear-gradient(135deg, #2a2f3a, #14171d)" }}
    >
      <span
        style={{
          backgroundImage: `linear-gradient(90deg, ${colors.played[1]} 0 50%, ${colors.unplayed[0]} 50% 100%)`,
          WebkitBackgroundClip: "text",
          backgroundClip: "text",
          color: "transparent",
        }}
      >
        歌词
      </span>
      {selected ? <RiCheckLine size={12} className="absolute top-0.5 right-0.5 text-white" /> : null}
    </span>
    {title}
  </button>
);

/** 设置页预览：用循环进度模拟播放中的效果 */
const usePreviewProgress = () =>
  useCallback(() => {
    const t = (Date.now() % PREVIEW_CYCLE_MS) / PREVIEW_CYCLE_MS;
    // 前 85% 时间推进进度，之后停在整行高亮
    return Math.min(1, t / 0.85);
  }, []);

/**
 * 「桌面歌词」设置页：开关、锁定、样式与实时预览。
 *
 * 直接读写全局设置（不经过常规设置的表单），托盘/桌面歌词工具栏修改后这里同步更新。
 */
const DesktopLyricsSettings = () => {
  const {
    showDesktopLyrics,
    desktopLyricsLocked,
    desktopLyricsFontSize,
    desktopLyricsStyle,
    primaryColor,
    fontFamily,
    update,
  } = useSettings(
    useShallow(s => ({
      showDesktopLyrics: s.showDesktopLyrics,
      desktopLyricsLocked: s.desktopLyricsLocked,
      desktopLyricsFontSize: s.desktopLyricsFontSize,
      desktopLyricsStyle: s.desktopLyricsStyle,
      primaryColor: s.primaryColor,
      fontFamily: s.fontFamily,
      update: s.update,
    })),
  );

  const style = useMemo(() => resolveDesktopLyricsStyle(desktopLyricsStyle), [desktopLyricsStyle]);
  const colors = useMemo(() => resolveDesktopLyricsColors(style, primaryColor), [style, primaryColor]);
  const getPreviewProgress = usePreviewProgress();

  const patchStyle = (patch: Partial<DesktopLyricsStyle>) => {
    update({ desktopLyricsStyle: { ...style, ...patch } });
  };

  const customColors = resolveDesktopLyricsColors({ ...style, colorPreset: "custom" });
  const themeColors = resolveDesktopLyricsColors({ ...style, colorPreset: "theme" }, primaryColor);
  // 预览字号按比例缩小，避免设置页里溢出
  const previewFontSize = Math.round(Math.min(desktopLyricsFontSize, 44));

  return (
    <div className="space-y-6">
      <SettingRow title="桌面歌词" description="在桌面显示置顶的悬浮歌词（系统托盘菜单也可开关）">
        <Switch
          disableAnimation
          isSelected={showDesktopLyrics}
          onValueChange={value => update({ showDesktopLyrics: value })}
        />
      </SettingRow>
      <SettingRow
        title="锁定桌面歌词"
        description="锁定后鼠标可穿透歌词窗口，悬停时出现解锁按钮；解锁后可拖动窗口、使用工具栏"
      >
        <Switch
          disableAnimation
          isSelected={desktopLyricsLocked}
          onValueChange={value => update({ desktopLyricsLocked: value })}
        />
      </SettingRow>

      {/* 预览 */}
      <div
        className="rounded-large w-full overflow-hidden px-6 py-5"
        style={{
          background:
            "radial-gradient(circle at 20% 20%, rgba(90,110,160,0.55), transparent 55%), radial-gradient(circle at 80% 80%, rgba(160,90,120,0.45), transparent 55%), #1b1e25",
        }}
      >
        <DesktopLyricsView
          fontSize={previewFontSize}
          style={style}
          colors={colors}
          text="風に揺れる街の灯り"
          romaji="kaze ni yureru machi no akari"
          translation="随风摇曳的街灯"
          nextText="遠く響くメロディー"
          hasTranslation
          hasRomaji
          index={0}
          playId="preview"
          getProgress={getPreviewProgress}
        />
      </div>

      <h2>布局</h2>
      <SettingRow title="显示行数" description="双行模式下当前行与下一行上下交错显示">
        <RadioGroup
          orientation="horizontal"
          value={style.lineMode}
          onValueChange={value => patchStyle({ lineMode: value as DesktopLyricsLineMode })}
        >
          <Radio value="single">单行</Radio>
          <Radio value="double">双行</Radio>
        </RadioGroup>
      </SettingRow>
      <SettingRow title="对齐方式" description="单行模式下歌词的对齐方式">
        <RadioGroup
          orientation="horizontal"
          isDisabled={style.lineMode === "double"}
          value={style.align}
          onValueChange={value => patchStyle({ align: value as DesktopLyricsAlign })}
        >
          <Radio value="left">居左</Radio>
          <Radio value="center">居中</Radio>
          <Radio value="right">居右</Radio>
        </RadioGroup>
      </SettingRow>
      <SettingRow title="显示翻译" description="歌曲有翻译时显示在原文下方">
        <Switch
          disableAnimation
          isSelected={style.showTranslation}
          onValueChange={value => patchStyle({ showTranslation: value })}
        />
      </SettingRow>
      <SettingRow title="显示罗马音" description="日语歌曲有罗马音时显示在原文下方">
        <Switch
          disableAnimation
          isSelected={style.showRomaji}
          onValueChange={value => patchStyle({ showRomaji: value })}
        />
      </SettingRow>

      <h2>文字</h2>
      <SettingRow title="字号" description="主歌词的文字大小">
        <div className="w-[300px]">
          <Slider
            showTooltip={false}
            size="sm"
            aria-label="桌面歌词字号"
            endContent={<span className="w-12 text-right">{desktopLyricsFontSize}px</span>}
            value={desktopLyricsFontSize}
            onChange={value => update({ desktopLyricsFontSize: Number(value) })}
            minValue={DESKTOP_LYRICS_FONT_SIZE_MIN}
            maxValue={DESKTOP_LYRICS_FONT_SIZE_MAX}
            step={1}
            classNames={{ thumb: "after:hidden" }}
          />
        </div>
      </SettingRow>
      <SettingRow title="翻译 / 罗马音字号" description="相对主歌词字号的比例">
        <div className="w-[300px]">
          <Slider
            showTooltip={false}
            size="sm"
            aria-label="翻译与罗马音字号比例"
            endContent={<span className="w-12 text-right">{Math.round(style.subScale * 100)}%</span>}
            value={style.subScale}
            onChange={value => patchStyle({ subScale: Number(value) })}
            minValue={DESKTOP_LYRICS_SUB_SCALE_MIN}
            maxValue={DESKTOP_LYRICS_SUB_SCALE_MAX}
            step={0.05}
            classNames={{ thumb: "after:hidden" }}
          />
        </div>
      </SettingRow>
      <SettingRow title="字体" description="默认跟随应用字体">
        <div className="flex items-center gap-2">
          {style.fontFamily ? (
            <Button size="sm" variant="light" onPress={() => patchStyle({ fontFamily: "" })}>
              跟随应用
            </Button>
          ) : null}
          <div className="w-[180px]">
            <FontSelect value={style.fontFamily || fontFamily} onChange={value => patchStyle({ fontFamily: value })} />
          </div>
        </div>
      </SettingRow>
      <SettingRow title="加粗">
        <Switch disableAnimation isSelected={style.bold} onValueChange={value => patchStyle({ bold: value })} />
      </SettingRow>
      <SettingRow title="文字效果" description="描边或阴影能让歌词在浅色背景上保持清晰">
        <RadioGroup
          orientation="horizontal"
          value={style.effect}
          onValueChange={value => patchStyle({ effect: value as DesktopLyricsEffect })}
        >
          <Radio value="stroke">描边</Radio>
          <Radio value="shadow">阴影</Radio>
          <Radio value="none">无</Radio>
        </RadioGroup>
      </SettingRow>
      <SettingRow title="不透明度" description="歌词文字整体的不透明度">
        <div className="w-[300px]">
          <Slider
            showTooltip={false}
            size="sm"
            aria-label="桌面歌词不透明度"
            endContent={<span className="w-12 text-right">{Math.round(style.opacity * 100)}%</span>}
            value={style.opacity}
            onChange={value => patchStyle({ opacity: Number(value) })}
            minValue={DESKTOP_LYRICS_OPACITY_MIN}
            maxValue={1}
            step={0.05}
            classNames={{ thumb: "after:hidden" }}
          />
        </div>
      </SettingRow>

      <h2>颜色</h2>
      <SettingRow title="进度高亮" description="按歌词行的时间进度逐渐填充已唱颜色">
        <Switch disableAnimation isSelected={style.karaoke} onValueChange={value => patchStyle({ karaoke: value })} />
      </SettingRow>
      <SettingRow
        title="翻译 / 罗马音同步高亮"
        description="翻译和罗马音与原文使用同一进度一起填充；关闭后保持未唱颜色"
      >
        <Switch
          disableAnimation
          isSelected={style.karaokeSubLines}
          onValueChange={value => patchStyle({ karaokeSubLines: value })}
        />
      </SettingRow>
      <div className="w-full space-y-2">
        <div className="text-medium font-medium">配色方案</div>
        <div className="flex flex-wrap gap-1">
          {desktopLyricsColorPresets.map(preset => (
            <Swatch
              key={preset.id}
              title={preset.name}
              colors={preset.colors}
              selected={style.colorPreset === preset.id}
              onPress={() => patchStyle({ colorPreset: preset.id })}
            />
          ))}
          <Swatch
            title="主题色"
            colors={themeColors}
            selected={style.colorPreset === "theme"}
            onPress={() => patchStyle({ colorPreset: "theme" })}
          />
          <Swatch
            title="自定义"
            colors={customColors}
            selected={style.colorPreset === "custom"}
            onPress={() => patchStyle({ colorPreset: "custom" })}
          />
        </div>
      </div>
      {style.colorPreset === "custom" ? (
        <SettingRow title="自定义颜色" description="已唱部分与未唱部分的颜色">
          <div className="flex items-center gap-3">
            <ColorPicker value={style.customPlayedColor} onChange={hex => patchStyle({ customPlayedColor: hex })}>
              <Button size="sm" variant="flat" startContent={<ColorDot color={style.customPlayedColor} />}>
                已唱
              </Button>
            </ColorPicker>
            <ColorPicker value={style.customUnplayedColor} onChange={hex => patchStyle({ customUnplayedColor: hex })}>
              <Button size="sm" variant="flat" startContent={<ColorDot color={style.customUnplayedColor} />}>
                未唱
              </Button>
            </ColorPicker>
          </div>
        </SettingRow>
      ) : null}

      <h2>窗口</h2>
      <SettingRow title="重置" description="恢复默认样式，或把歌词窗口移回主屏幕底部">
        <div className="flex items-center gap-2">
          <Button
            size="sm"
            variant="flat"
            onPress={() =>
              update({
                desktopLyricsStyle: defaultDesktopLyricsStyle,
                desktopLyricsFontSize: defaultAppSettings.desktopLyricsFontSize,
              })
            }
          >
            重置样式
          </Button>
          <Button size="sm" variant="flat" onPress={() => window.electron.resetDesktopLyricsPosition()}>
            重置位置
          </Button>
        </div>
      </SettingRow>
    </div>
  );
};

const ColorDot = ({ color }: { color: string }) => (
  <span className="h-3.5 w-3.5 rounded-full border border-black/20" style={{ backgroundColor: color }} />
);

export default DesktopLyricsSettings;
