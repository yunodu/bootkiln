<script lang="ts">
  import type { Phase, Progress } from "./api";
  import { bytes, rate, roundEta } from "./format";
  import { t } from "./texts";

  let {
    steps,
    progress,
    cancelling,
    context,
    onCancel,
  }: { steps: Phase[]; progress: Progress | null; cancelling: boolean; context: string; onCancel: () => void } =
    $props();

  /** Throughput is measured over this many recent seconds; USB writes are bursty. */
  const RATE_WINDOW_S = 10;
  /** No estimate until the window holds this much history. */
  const MIN_HISTORY_S = 3;
  /** The shown speed and time left change at most this often. */
  const DISPLAY_INTERVAL_MS = 1000;

  let samples: { phase: Phase; done: number; at: number }[] = [];
  let lastShown = 0;
  /** Plain copy of `eta`, so the effect does not depend on what it writes. */
  let shownEta: number | null = null;
  let speed = $state(0);
  let eta = $state<number | null>(null);

  $effect(() => {
    const p = progress;
    if (!p) return;
    const now = performance.now();
    if (samples.length > 0 && (samples[0].phase !== p.phase || p.done < samples[samples.length - 1].done)) {
      samples = [];
      speed = 0;
      eta = shownEta = null;
    }
    samples.push({ phase: p.phase, done: p.done, at: now });
    while (samples.length > 2 && now - samples[1].at > RATE_WINDOW_S * 1000) samples.shift();

    const first = samples[0];
    const span = (now - first.at) / 1000;
    if (span < MIN_HISTORY_S || now - lastShown < DISPLAY_INTERVAL_MS) return;
    lastShown = now;
    speed = (p.done - first.done) / span;
    const remaining = speed > 0 ? (p.total - p.done) / speed : null;
    // Let the estimate fall freely but rise only slowly, so it does not bounce.
    shownEta =
      remaining === null || shownEta === null || remaining < shownEta
        ? remaining
        : shownEta + (remaining - shownEta) * 0.25;
    eta = shownEta;
  });

  let current = $derived(progress?.phase ?? steps[0]);
  let currentIndex = $derived(steps.indexOf(current));
  let fraction = $derived(progress && progress.total > 0 ? progress.done / progress.total : 0);
  let indeterminate = $derived(current === "formatting" || !progress);
</script>

<div class="panel fade-in">
  <div class="context" title={context}>{context}</div>

  <ol class="steps">
    {#each steps as step, i (step + i)}
      <li class:done={i < currentIndex} class:active={i === currentIndex}>
        <span class="dot"></span>{t.phases[step]}
      </li>
    {/each}
  </ol>

  <div class="headline">
    <span class="phase">{t.phases[current]}</span>
    {#if !indeterminate}<span class="percent">{Math.floor(fraction * 100)}%</span>{/if}
  </div>

  <div class="bar" class:indeterminate>
    <div class="fill" style:width={indeterminate ? "35%" : `${fraction * 100}%`}></div>
  </div>

  <div class="stats">
    {#if current === "formatting"}
      <span>{t.formatting}</span>
    {:else if progress}
      <span>{t.progressOf(bytes(progress.done), bytes(progress.total))}</span>
      <span>
        {#if speed > 0}{rate(speed)}{/if}
        {#if eta !== null}&nbsp;· {t.timeLeft(roundEta(eta))}{/if}
      </span>
    {/if}
  </div>

  <button class="btn cancel" onclick={onCancel} disabled={cancelling}>
    {cancelling ? t.cancelling : t.cancel}
  </button>
</div>

<style>
  .panel {
    display: flex;
    flex-direction: column;
    gap: 14px;
    padding: 20px;
    border-radius: var(--radius-card);
    background: var(--card);
    border: 1px solid var(--card-border);
    box-shadow: var(--shadow-card);
  }

  .context {
    font-size: 12.5px;
    color: var(--text-faint);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .steps {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 14px;
    margin: 0;
    padding: 0;
    list-style: none;
    font-size: 12.5px;
    color: var(--text-faint);
  }

  .steps li {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }

  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--track);
  }

  .steps li.done {
    color: var(--text-muted);
  }

  .steps li.done .dot {
    background: var(--ok);
  }

  .steps li.active {
    color: var(--text);
    font-weight: 600;
  }

  .steps li.active .dot {
    background: var(--accent);
    box-shadow: 0 0 0 3px var(--accent-soft);
  }

  .headline {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
  }

  .phase {
    font-family: var(--font-display);
    font-size: 22px;
    font-weight: 600;
  }

  .percent {
    font-family: var(--font-display);
    font-size: 22px;
    font-variant-numeric: tabular-nums;
    color: var(--accent);
  }

  .bar {
    position: relative;
    height: 10px;
    border-radius: 5px;
    background: var(--track);
    overflow: hidden;
  }

  .fill {
    height: 100%;
    border-radius: 5px;
    background: var(--accent-gradient);
    box-shadow: 0 0 12px rgba(240, 98, 45, 0.5);
    transition: width 0.25s linear;
  }

  .indeterminate .fill {
    position: absolute;
    animation: slide 1.3s ease-in-out infinite;
  }

  @keyframes slide {
    from {
      left: -35%;
    }
    to {
      left: 100%;
    }
  }

  .stats {
    display: flex;
    justify-content: space-between;
    min-height: 20px;
    font-size: 13px;
    font-variant-numeric: tabular-nums;
    color: var(--text-muted);
  }

  .cancel {
    align-self: flex-end;
  }
</style>
