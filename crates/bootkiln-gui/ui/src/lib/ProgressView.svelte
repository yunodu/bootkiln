<script lang="ts">
  import type { Phase, Progress } from "./api";
  import { bytes, duration, rate } from "./format";
  import { t } from "./texts";

  let {
    steps,
    progress,
    cancelling,
    context,
    onCancel,
  }: { steps: Phase[]; progress: Progress | null; cancelling: boolean; context: string; onCancel: () => void } =
    $props();

  // Smoothed throughput of the current phase.
  let speed = $state(0);
  let sample: { phase: Phase; done: number; at: number } | null = null;

  $effect(() => {
    const p = progress;
    if (!p) return;
    const now = performance.now();
    if (!sample || sample.phase !== p.phase || p.done < sample.done) {
      sample = { phase: p.phase, done: p.done, at: now };
      speed = 0;
      return;
    }
    const dt = (now - sample.at) / 1000;
    if (dt >= 0.5) {
      const instant = (p.done - sample.done) / dt;
      speed = speed === 0 ? instant : speed * 0.7 + instant * 0.3;
      sample = { phase: p.phase, done: p.done, at: now };
    }
  });

  let current = $derived(progress?.phase ?? steps[0]);
  let currentIndex = $derived(steps.indexOf(current));
  let fraction = $derived(progress && progress.total > 0 ? progress.done / progress.total : 0);
  let eta = $derived(progress && speed > 0 ? (progress.total - progress.done) / speed : null);
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
        {#if eta !== null}&nbsp;· {t.progressLeft(duration(eta))}{/if}
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
