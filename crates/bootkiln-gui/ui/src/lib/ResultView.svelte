<script lang="ts">
  import type { Note } from "./api";
  import Icon from "./Icon.svelte";
  import { t } from "./texts";

  type Outcome =
    | { kind: "done"; image: string; drive: string; note: Note }
    | { kind: "cancelled" }
    | { kind: "failed"; error: string };

  let {
    outcome,
    ejectState,
    onEject,
    onClose,
  }: {
    outcome: Outcome;
    ejectState: "idle" | "busy" | "done" | { error: string };
    onEject: () => void;
    onClose: () => void;
  } = $props();
</script>

<div class="panel fade-in {outcome.kind}">
  {#if outcome.kind === "done"}
    <div class="badge ok"><Icon name="check" size={28} /></div>
    <h2>{t.doneTitle}</h2>
    <p>{t.doneBody(outcome.image, outcome.drive)}</p>
    <div class="notice"><Icon name="info" size={16} /><span>{t.notes[outcome.note]}</span></div>
    {#if typeof ejectState === "object"}
      <div class="notice error"><Icon name="alert" size={16} /><span>{ejectState.error}</span></div>
    {/if}
    <div class="actions">
      {#if ejectState === "done"}
        <span class="ejected"><Icon name="check" size={16} />{t.ejected}</span>
      {:else}
        <button class="btn" onclick={onEject} disabled={ejectState === "busy"}>
          <Icon name="eject" size={16} />{ejectState === "busy" ? t.ejecting : t.eject}
        </button>
      {/if}
      <button class="btn primary" onclick={onClose}>{t.doneButton}</button>
    </div>
  {:else if outcome.kind === "cancelled"}
    <div class="badge warn"><Icon name="alert" size={26} /></div>
    <h2>{t.cancelledTitle}</h2>
    <p>{t.cancelledBody}</p>
    <div class="actions"><button class="btn primary" onclick={onClose}>{t.back}</button></div>
  {:else}
    <div class="badge error"><Icon name="close" size={26} /></div>
    <h2>{t.failedTitle}</h2>
    <p class="error-text">{outcome.error}</p>
    <div class="actions"><button class="btn primary" onclick={onClose}>{t.back}</button></div>
  {/if}
</div>

<style>
  .panel {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 24px 20px 20px;
    border-radius: var(--radius-card);
    background: var(--card);
    border: 1px solid var(--card-border);
    box-shadow: var(--shadow-card);
  }

  .badge {
    display: grid;
    place-items: center;
    width: 52px;
    height: 52px;
    border-radius: 16px;
    animation: pop 0.35s var(--ease);
  }

  .badge.ok {
    color: #fff;
    background: linear-gradient(135deg, #3ccf7a, #0f9d58);
    box-shadow: 0 8px 20px rgba(15, 157, 88, 0.35);
  }

  .badge.warn {
    color: var(--warn);
    background: var(--warn-bg);
  }

  .badge.error {
    color: var(--error);
    background: var(--error-bg);
  }

  h2 {
    margin: 4px 0 0;
    font-family: var(--font-display);
    font-size: 22px;
    font-weight: 600;
  }

  p {
    margin: 0;
    color: var(--text-muted);
  }

  .error-text {
    color: var(--error);
    user-select: text;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    align-items: center;
    gap: 8px;
    margin-top: 6px;
  }

  .ejected {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    margin-right: auto;
    color: var(--ok);
    font-weight: 600;
  }

  @keyframes pop {
    from {
      transform: scale(0.6);
      opacity: 0;
    }
  }
</style>
