<script lang="ts">
  import type { Snippet } from "svelte";

  let {
    checked = $bindable(false),
    label,
    hint,
    disabled = false,
    children,
  }: { checked?: boolean; label: string; hint?: string; disabled?: boolean; children?: Snippet } = $props();
</script>

<div class="row" class:disabled>
  <label>
    <span class="text">
      <span class="label">{label}</span>
      {#if hint}<span class="hint">{hint}</span>{/if}
    </span>
    <input type="checkbox" role="switch" bind:checked {disabled} />
    <span class="switch" aria-hidden="true"><span class="knob"></span></span>
  </label>
  {#if checked && children}
    <div class="extra fade-in">{@render children()}</div>
  {/if}
</div>

<style>
  .row {
    padding: 8px 0;
  }

  .row:not(:first-of-type) {
    border-top: 1px solid var(--card-border);
  }

  label {
    display: flex;
    align-items: center;
    gap: 14px;
    cursor: pointer;
  }

  .disabled label {
    cursor: default;
    opacity: 0.5;
  }

  .text {
    flex: 1;
    display: flex;
    flex-direction: column;
  }

  .label {
    font-size: 14px;
  }

  .hint {
    font-size: 12.5px;
    color: var(--text-faint);
  }

  input {
    position: absolute;
    opacity: 0;
    pointer-events: none;
  }

  .switch {
    flex: none;
    position: relative;
    width: 40px;
    height: 22px;
    border-radius: 11px;
    background: var(--track);
    border: 1px solid var(--control-border);
    transition: background 0.2s var(--ease), border-color 0.2s var(--ease);
  }

  .knob {
    position: absolute;
    top: 3px;
    left: 3px;
    width: 14px;
    height: 14px;
    border-radius: 50%;
    background: var(--text-muted);
    transition: transform 0.2s var(--ease), background 0.2s var(--ease), width 0.15s var(--ease);
  }

  label:active .knob {
    width: 17px;
  }

  input:checked + .switch {
    background: var(--accent);
    border-color: transparent;
  }

  input:checked + .switch .knob {
    background: #fff;
    transform: translateX(18px);
  }

  input:focus-visible + .switch {
    outline: 2px solid var(--accent-ring);
    outline-offset: 2px;
  }

  .extra {
    margin-top: 8px;
  }
</style>
