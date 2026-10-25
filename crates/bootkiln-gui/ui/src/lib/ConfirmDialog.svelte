<script lang="ts">
  import Icon from "./Icon.svelte";
  import { t } from "./texts";

  let { drive, onCancel, onConfirm }: { drive: string; onCancel: () => void; onConfirm: () => void } = $props();
</script>

<svelte:window onkeydown={(e) => e.key === "Escape" && onCancel()} />

<div class="backdrop" role="presentation" onclick={onCancel}></div>
<div class="dialog" role="alertdialog" aria-modal="true" aria-labelledby="confirm-title">
  <div class="icon"><Icon name="alert" size={22} /></div>
  <h2 id="confirm-title">{t.confirmTitle}</h2>
  <p>{t.confirmBody(drive)}</p>
  <div class="actions">
    <button class="btn" onclick={onCancel}>{t.confirmCancel}</button>
    <!-- svelte-ignore a11y_autofocus -->
    <button class="btn danger" onclick={onConfirm} autofocus>{t.confirmGo}</button>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: var(--backdrop);
    backdrop-filter: blur(3px);
    animation: appear 0.2s var(--ease);
  }

  .dialog {
    position: fixed;
    left: 50%;
    top: 50%;
    width: min(380px, calc(100% - 40px));
    transform: translate(-50%, -50%);
    padding: 22px 22px 18px;
    border-radius: 16px;
    background: var(--surface);
    border: 1px solid var(--card-border);
    box-shadow: 0 24px 60px rgba(0, 0, 0, 0.35);
    animation: pop 0.22s var(--ease);
  }

  .icon {
    display: grid;
    place-items: center;
    width: 40px;
    height: 40px;
    border-radius: 12px;
    color: var(--error);
    background: var(--error-bg);
  }

  h2 {
    margin: 14px 0 6px;
    font-family: var(--font-display);
    font-size: 18px;
    font-weight: 600;
  }

  p {
    margin: 0 0 18px;
    color: var(--text-muted);
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }

  @keyframes appear {
    from {
      opacity: 0;
    }
  }

  @keyframes pop {
    from {
      opacity: 0;
      transform: translate(-50%, -46%) scale(0.97);
    }
  }
</style>
