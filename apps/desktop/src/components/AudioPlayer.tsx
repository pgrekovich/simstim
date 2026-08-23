import {
  forwardRef,
  useEffect,
  useImperativeHandle,
  useRef,
  useState,
} from "react";
import { Pause, Play } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Slider } from "@/components/ui/slider";

/** Playback speeds cycled by the rate button, in order. */
const RATES = [1, 1.25, 1.5, 2];

export interface AudioPlayerHandle {
  /** Seeks to `offsetSecs` and starts playback (transcript line clicks). */
  seek: (offsetSecs: number) => void;
}

interface AudioPlayerProps {
  src: string;
  /** Rendered at the right edge of the bar (the Re-transcribe button). */
  trailing?: React.ReactNode;
}

function formatClock(secs: number): string {
  if (!Number.isFinite(secs)) return "0:00";
  const whole = Math.floor(secs);
  const h = Math.floor(whole / 3600);
  const m = Math.floor((whole % 3600) / 60);
  const s = whole % 60;
  const mm = h > 0 ? String(m).padStart(2, "0") : String(m);
  return `${h > 0 ? `${h}:` : ""}${mm}:${String(s).padStart(2, "0")}`;
}

/**
 * A compact custom bar around a hidden `<audio>` element: play/pause, a seek
 * slider, elapsed/total time and a playback-rate cycler. Replaces the native
 * `controls` chrome, which reads as a foreign embed inside the app (owner
 * design review). Kept a single row on purpose: it pins to the bottom of
 * the Transcript tab, where a chat input will later have to share space.
 */
export const AudioPlayer = forwardRef<AudioPlayerHandle, AudioPlayerProps>(
  function AudioPlayer({ src, trailing }, ref) {
    const audioRef = useRef<HTMLAudioElement>(null);
    const [playing, setPlaying] = useState(false);
    const [duration, setDuration] = useState(0);
    const [position, setPosition] = useState(0);
    const [rateIndex, setRateIndex] = useState(0);
    // While the user drags the slider, the drag position wins over
    // timeupdate ticks; commit seeks the element on release.
    const [dragging, setDragging] = useState<number | null>(null);

    useImperativeHandle(ref, () => ({
      seek: (offsetSecs: number) => {
        const audio = audioRef.current;
        if (!audio) return;
        audio.currentTime = offsetSecs;
        void audio.play();
      },
    }));

    useEffect(() => {
      const audio = audioRef.current;
      if (!audio) return;
      const onTime = () => setPosition(audio.currentTime);
      const onMeta = () => setDuration(audio.duration);
      const onPlay = () => setPlaying(true);
      const onPause = () => setPlaying(false);
      audio.addEventListener("timeupdate", onTime);
      audio.addEventListener("loadedmetadata", onMeta);
      audio.addEventListener("play", onPlay);
      audio.addEventListener("pause", onPause);
      audio.addEventListener("ended", onPause);
      return () => {
        audio.removeEventListener("timeupdate", onTime);
        audio.removeEventListener("loadedmetadata", onMeta);
        audio.removeEventListener("play", onPlay);
        audio.removeEventListener("pause", onPause);
        audio.removeEventListener("ended", onPause);
      };
    }, []);

    function togglePlay() {
      const audio = audioRef.current;
      if (!audio) return;
      if (playing) {
        audio.pause();
      } else {
        void audio.play();
      }
    }

    function cycleRate() {
      const audio = audioRef.current;
      const next = (rateIndex + 1) % RATES.length;
      setRateIndex(next);
      if (audio) audio.playbackRate = RATES[next];
    }

    const shown = dragging ?? position;

    return (
      <div className="flex shrink-0 items-center gap-3 rounded-lg border bg-card px-3 py-2">
        <audio ref={audioRef} src={src} preload="metadata" className="hidden" />

        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="size-8 shrink-0"
          aria-label={playing ? "Pause" : "Play"}
          onClick={togglePlay}
        >
          {playing ? <Pause className="size-4" /> : <Play className="size-4" />}
        </Button>

        <span className="w-12 shrink-0 text-right text-xs tabular-nums text-muted-foreground">
          {formatClock(shown)}
        </span>

        <Slider
          value={[shown]}
          min={0}
          max={duration || 1}
          step={0.1}
          onValueChange={([v]) => setDragging(v)}
          onValueCommit={([v]) => {
            setDragging(null);
            const audio = audioRef.current;
            if (audio) audio.currentTime = v;
          }}
          className="min-w-0 flex-1"
        />

        <span className="w-12 shrink-0 text-xs tabular-nums text-muted-foreground">
          {formatClock(duration)}
        </span>

        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="w-12 shrink-0 text-xs tabular-nums text-muted-foreground"
          aria-label="Playback speed"
          onClick={cycleRate}
        >
          {RATES[rateIndex]}x
        </Button>

        {trailing}
      </div>
    );
  }
);
