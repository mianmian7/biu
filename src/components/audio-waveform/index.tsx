import { useEffect, useRef } from "react";

import { audio as audioElement } from "@/store/play-list";

interface AudioWaveformProps {
  width?: number;
  height?: number;
  barCount?: number;
  barColor?: string;
}

// Global AudioContext singleton to prevent multiple MediaElementSourceNode creation
let audioContext: AudioContext | null = null;
let analyser: AnalyserNode | null = null;
let source: MediaElementAudioSourceNode | null = null;

function resumeAudioContext() {
  if (audioContext) void audioContext.resume().catch(() => {});
}

function suspendAudioContext() {
  if (audioContext) void audioContext.suspend().catch(() => {});
}

function initAudioContext() {
  if (audioContext) return;
  const AudioContextClass = window.AudioContext || (window as any).webkitAudioContext;
  if (!AudioContextClass) return;
  const context: AudioContext = new AudioContextClass();
  const node = context.createAnalyser();
  node.fftSize = 512;
  try {
    source = context.createMediaElementSource(audioElement);
    source.connect(context.destination);
  } catch (error) {
    void context.close().catch(() => {});
    console.warn("MediaElementSourceNode creation failed:", error);
    return;
  }
  audioContext = context;
  analyser = node;

  // The media element stays routed through this context after the canvas unmounts.
  audioElement.addEventListener("play", resumeAudioContext);
  audioElement.addEventListener("pause", suspendAudioContext);
  audioElement.addEventListener("ended", suspendAudioContext);
}

/**
 * 音频波形可视化组件
 * 使用 Web Audio API 实现动态频谱效果
 */
const AudioWaveform = ({ width = 56, height = 56, barCount = 40, barColor = "currentColor" }: AudioWaveformProps) => {
  const canvasRef = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || !audioElement) return;

    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    initAudioContext();
    const waveformAnalyser = analyser;
    const waveformSource = source;
    if (!waveformAnalyser || !waveformSource) return;
    waveformSource.connect(waveformAnalyser);
    const bufferLength = waveformAnalyser.frequencyBinCount;
    const dataArray = new Uint8Array(bufferLength);
    let animationId = 0;

    const handlePlay = () => {
      if (!animationId) render();
    };

    const handlePause = () => {
      cancelAnimationFrame(animationId);
      animationId = 0;
    };

    const draw = () => {
      waveformAnalyser.getByteFrequencyData(dataArray);

      ctx.clearRect(0, 0, width, height);

      const computedBarWidth = width / barCount;
      const barGap = computedBarWidth * 0.2;
      const barWidth = computedBarWidth - barGap;

      // Focus on the lower 60% of the frequency spectrum (most music energy)
      // With fftSize=512, bufferLength=256.
      // 0.6 * 256 * (44100/512) ≈ 13kHz coverage
      const usefulBufferLength = Math.floor(bufferLength * 0.6);

      // Draw bars
      for (let i = 0; i < barCount; i++) {
        // Map bar index to frequency data index
        const dataIndex = Math.floor((i / barCount) * usefulBufferLength);
        let value = dataArray[dataIndex];

        // Boost high frequencies (right side) as they are naturally quieter
        // Linear boost from 1x to 2x
        const boost = 1 + i / barCount;
        value = Math.min(255, value * boost);

        // Calculate bar height based on value (0-255)
        // Ensure a minimum height (e.g., 2px) to show a "base" row like PotPlayer
        const barHeight = Math.max((value / 255) * height, 2);

        const x = i * computedBarWidth;
        const y = height - barHeight;

        // Set color
        ctx.fillStyle = barColor === "currentColor" ? "#666" : barColor;

        // Draw rounded bar (simulated)
        ctx.beginPath();
        // Use rect for simplicity, or roundRect if supported
        if (ctx.roundRect) {
          ctx.roundRect(x, y, barWidth, barHeight, 2);
        } else {
          ctx.fillRect(x, y, barWidth, barHeight);
        }
        ctx.fill();
      }
    };

    const render = () => {
      draw();
      animationId = requestAnimationFrame(render);
    };

    audioElement.addEventListener("play", handlePlay);
    audioElement.addEventListener("pause", handlePause);
    audioElement.addEventListener("ended", handlePause);

    // Initialize state
    if (audioElement.paused) {
      draw();
      suspendAudioContext();
    } else {
      resumeAudioContext();
      render();
    }

    return () => {
      handlePause();
      waveformSource.disconnect(waveformAnalyser);
      if (audioElement.paused) suspendAudioContext();
      audioElement.removeEventListener("play", handlePlay);
      audioElement.removeEventListener("pause", handlePause);
      audioElement.removeEventListener("ended", handlePause);
    };
  }, [width, height, barCount, barColor]);

  return <canvas ref={canvasRef} width={width} height={height} />;
};

export default AudioWaveform;
