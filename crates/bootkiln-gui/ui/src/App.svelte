<script lang="ts">
  import { onMount } from "svelte";
  import logo from "../../icons/icon.svg?url";
  import * as api from "./lib/api";
  import AdvancedCard from "./lib/AdvancedCard.svelte";
  import CatalogSheet, { type Resolved } from "./lib/CatalogSheet.svelte";
  import ConfirmDialog from "./lib/ConfirmDialog.svelte";
  import DriveCard from "./lib/DriveCard.svelte";
  import Icon from "./lib/Icon.svelte";
  import ImageCard, { type DownloadView } from "./lib/ImageCard.svelte";
  import ProgressView from "./lib/ProgressView.svelte";
  import ResultView from "./lib/ResultView.svelte";
  import WindowsCard from "./lib/WindowsCard.svelte";
  import { bytes, fileName } from "./lib/format";
  import { t } from "./lib/texts";

  type Outcome =
    | { kind: "done"; image: string; drive: string; note: api.Note }
    | { kind: "cancelled" }
    | { kind: "failed"; error: string };

  const noWindowsOptions: api.WindowsOptions = {
    skipHardwareChecks: false,
    skipOnlineAccount: false,
    localAccount: null,
    copyRegion: false,
    skipPrivacyQuestions: false,
    disableBitlocker: false,
  };

  let drives = $state<api.Drive[]>([]);
  let driveNumber = $state<number | null>(null);

  let imagePath = $state<string | null>(null);
  let info = $state<api.ImageInfo | null>(null);
  let analyzing = $state(false);
  let imageError = $state<string | null>(null);
  let dragOver = $state(false);

  let mode = $state<api.Mode>("auto");
  let verify = $state(true);
  let winOptions = $state<api.WindowsOptions>({ ...noWindowsOptions });
  let localAccount = $state(false);
  let accountName = $state("");

  let view = $state<"form" | "confirm" | "running" | "result">("form");
  let checking = $state(false);
  let checkError = $state<string | null>(null);
  let steps = $state<api.Phase[]>([]);
  let progress = $state<api.Progress | null>(null);
  let cancelling = $state(false);
  let outcome = $state<Outcome | null>(null);
  let ejectState = $state<"idle" | "busy" | "done" | { error: string }>("idle");

  let catalogOpen = $state(false);
  let entries = $state<api.CatalogEntry[]>([]);
  let resolved = $state<Record<string, Resolved>>({});
  let windowsLanguage = $state("");
  let download = $state<DownloadView | null>(null);
  let update = $state<api.Update | null>(null);
  let downloading: { release: api.Release; entry: api.CatalogEntry } | null = null;

  let drive = $derived(drives.find((d) => d.number === driveNumber) ?? null);
  let driveLabel = $derived(drive ? `${drive.model || "the drive"} (${drive.mountPoints[0] ?? bytes(drive.sizeBytes)})` : "");
  let ready = $derived(
    drive !== null &&
      info !== null &&
      download === null &&
      !analyzing &&
      !imageError &&
      !(localAccount && !accountName.trim()),
  );
  let startLabel = $derived(checking ? t.checking : !drive ? t.pickDrive : !imagePath ? t.pickImage : t.start);

  function setDrives(list: api.Drive[]) {
    drives = list;
    if (driveNumber !== null && !list.some((d) => d.number === driveNumber)) driveNumber = null;
    if (driveNumber === null && list.length === 1) driveNumber = list[0].number;
  }

  // Re-read the image whenever it or the mode changes; stale answers are dropped.
  let analysisToken = 0;
  $effect(() => {
    const path = imagePath;
    const m = mode;
    if (!path) return;
    const token = ++analysisToken;
    analyzing = true;
    imageError = null;
    api
      .analyzeImage(path, m)
      .then((result) => {
        if (token === analysisToken) info = result;
      })
      .catch((e) => {
        if (token === analysisToken) {
          info = null;
          imageError = String(e);
        }
      })
      .finally(() => {
        if (token === analysisToken) analyzing = false;
      });
  });

  function chooseImage(path: string) {
    if (view !== "form") return;
    info = null;
    checkError = null;
    imagePath = path;
  }

  async function browse() {
    const path = await api.pickImage();
    if (path) chooseImage(path);
  }

  function request(): api.WriteRequest {
    const windows = info?.windows
      ? { ...winOptions, localAccount: localAccount ? accountName.trim() : null }
      : { ...noWindowsOptions };
    return { image: imagePath!, drive: driveNumber!, mode, force: false, verify, windows };
  }

  async function start() {
    checking = true;
    checkError = null;
    try {
      await api.checkWrite(request());
      view = "confirm";
    } catch (e) {
      checkError = String(e);
    } finally {
      checking = false;
    }
  }

  async function confirmed() {
    const ntfs = info?.method === "iso" && info.fileSystem === "ntfs";
    const all: (api.Phase | false)[] = ntfs
      ? ["writing", verify && "verifying", "formatting", "copying", verify && "verifyingFiles"]
      : ["writing", verify && "verifying"];
    steps = all.filter((s): s is api.Phase => s !== false);
    progress = null;
    cancelling = false;
    view = "running";
    try {
      await api.startWrite(request());
    } catch (e) {
      outcome = { kind: "failed", error: String(e) };
      view = "result";
    }
  }

  function finished(f: api.Finished) {
    if (f.ok && f.note) outcome = { kind: "done", image: fileName(imagePath ?? ""), drive: driveLabel, note: f.note };
    else if (f.cancelled) outcome = { kind: "cancelled" };
    else outcome = { kind: "failed", error: f.error ?? "unknown error" };
    ejectState = "idle";
    view = "result";
  }

  async function cancel() {
    cancelling = true;
    await api.cancelWrite();
  }

  async function eject() {
    if (driveNumber === null) return;
    ejectState = "busy";
    try {
      await api.ejectDrive(driveNumber);
      ejectState = "done";
    } catch (e) {
      ejectState = { error: String(e) };
    }
  }

  function resolveEntry(entry: api.CatalogEntry) {
    resolved[entry.id] = { state: "loading" };
    api
      .resolveRelease(entry.id, entry.windows ? windowsLanguage : null)
      .then((release) => (resolved[entry.id] = { state: "ok", release }))
      .catch((e) => (resolved[entry.id] = { state: "error", error: String(e) }));
  }

  async function openCatalog() {
    catalogOpen = true;
    if (entries.length === 0) {
      [entries, windowsLanguage] = await Promise.all([api.catalogEntries(), api.defaultWindowsLanguage()]);
    }
    // Errors are retried on every opening; good answers are kept for the session.
    entries.filter((e) => resolved[e.id]?.state !== "ok").forEach(resolveEntry);
  }

  // A new Windows language needs a new link and checksum.
  $effect(() => {
    const language = windowsLanguage;
    const windows = entries.find((e) => e.windows);
    const current = windows ? resolved[windows.id] : undefined;
    if (windows && current?.state === "ok" && current.release.language !== language) resolveEntry(windows);
  });

  async function beginDownload(release: api.Release, entry: api.CatalogEntry) {
    catalogOpen = false;
    downloading = { release, entry };
    download = {
      name: entry.name,
      version: release.version,
      color: entry.color,
      status: "running",
      stage: "downloading",
      done: 0,
      total: release.size ?? 0,
      error: null,
    };
    try {
      await api.startDownload(release);
    } catch (e) {
      download = { ...download, status: "failed", error: String(e) };
    }
  }

  async function resumeDownload() {
    if (!downloading || !download) return;
    download = { ...download, status: "running", error: null };
    let release = downloading.release;
    // Microsoft's links expire after a day; fetch a fresh one for the same file.
    if (downloading.entry.windows) {
      try {
        release = await api.resolveRelease(downloading.entry.id, release.language);
        downloading = { ...downloading, release };
      } catch (e) {
        download = { ...download, status: "failed", error: String(e) };
        return;
      }
    }
    beginDownload(release, downloading.entry);
  }

  async function cancelDownload() {
    if (download?.status === "running") {
      await api.cancelDownload();
    } else if (downloading) {
      await api.discardDownload(downloading.release).catch(() => {});
      download = null;
      downloading = null;
    }
  }

  function downloadFinished(f: api.DownloadFinished) {
    if (!download) return;
    if (f.path) {
      download = null;
      downloading = null;
      chooseImage(f.path);
    } else if (f.paused) {
      download = { ...download, status: "paused" };
    } else if (f.cancelled) {
      download = null;
      downloading = null;
    } else {
      download = { ...download, status: "failed", error: f.error };
    }
  }

  onMount(() => {
    api.mica().then((on) => document.documentElement.classList.toggle("mica", on));
    // Notify only; offline or a GitHub hiccup just means no notice.
    api.checkUpdate().then((u) => (update = u), () => {});
    api.listDrives().then(setDrives, (e) => (checkError = String(e)));
    const subscriptions = [
      api.onDrivesChanged(setDrives),
      api.onProgress((p) => (progress = p)),
      api.onFinished(finished),
      api.onFileDrop(
        (over) => (dragOver = over && view === "form"),
        (path) => chooseImage(path),
      ),
      api.onDownloadProgress((p) => {
        if (download?.status === "running") download = { ...download, stage: p.stage, done: p.done, total: p.total };
      }),
      api.onDownloadFinished(downloadFinished),
    ];
    return () => subscriptions.forEach((s) => s.then((unlisten) => unlisten()));
  });
</script>

<div class="shell">
  <header>
    <img src={logo} alt="" width="34" height="34" />
    <div>
      <h1>bootkiln</h1>
      <p>{t.appTagline}</p>
    </div>
    {#if update}
      {@const found = update}
      <button class="update fade-in" onclick={() => api.openReleasePage(found.url)}>
        {t.updateAvailable(found.version)}
      </button>
    {/if}
  </header>

  <main class:centered={view === "running" || view === "result"}>
    {#if view === "running"}
      <ProgressView {steps} {progress} {cancelling} context={`${fileName(imagePath ?? "")} → ${driveLabel}`} onCancel={cancel} />
    {:else if view === "result" && outcome}
      <ResultView {outcome} {ejectState} onEject={eject} onClose={() => (view = "form")} />
    {:else}
      <DriveCard {drives} bind:selected={driveNumber} />
      <ImageCard
        path={imagePath}
        {info}
        {analyzing}
        error={imageError}
        {dragOver}
        {download}
        onBrowse={browse}
        onCatalog={openCatalog}
        onPause={() => api.pauseDownload()}
        onResume={resumeDownload}
        onCancelDownload={cancelDownload}
      />
      {#if info?.windows}
        <WindowsCard windows={info.windows} bind:options={winOptions} bind:localAccount bind:accountName />
      {/if}
      <AdvancedCard bind:mode bind:verify />
    {/if}
  </main>

  {#if view === "form" || view === "confirm"}
    <footer>
      {#if checkError}
        <div class="notice error fade-in"><Icon name="alert" size={16} /><span>{checkError}</span></div>
      {/if}
      <button class="btn primary large" disabled={!ready || checking} onclick={start}>
        {#if checking}<span class="spinner light"></span>{/if}{startLabel}
      </button>
    </footer>
  {/if}
</div>

{#if view === "confirm"}
  <ConfirmDialog drive={driveLabel} onCancel={() => (view = "form")} onConfirm={confirmed} />
{/if}

{#if catalogOpen}
  <CatalogSheet
    {entries}
    {resolved}
    bind:language={windowsLanguage}
    onDownload={beginDownload}
    onClose={() => (catalogOpen = false)}
  />
{/if}

<style>
  .shell {
    display: flex;
    flex-direction: column;
    height: 100%;
  }

  header {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 18px 20px 10px;
  }

  header img {
    border-radius: 9px;
    box-shadow: 0 4px 14px rgba(224, 74, 26, 0.35);
  }

  h1 {
    margin: 0;
    font-family: var(--font-display);
    font-size: 20px;
    font-weight: 700;
    letter-spacing: -0.01em;
  }

  header p {
    margin: 0;
    font-size: 12.5px;
    color: var(--text-faint);
  }

  .update {
    margin-left: auto;
    padding: 5px 12px;
    border: none;
    border-radius: 999px;
    font-size: 12.5px;
    font-weight: 600;
    color: var(--accent);
    background: var(--accent-soft);
  }

  .update:hover {
    filter: brightness(1.1);
  }

  main {
    flex: 1;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 8px 20px 16px;
  }

  main.centered {
    justify-content: center;
    padding-bottom: 60px;
  }

  footer {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 12px 20px 18px;
    border-top: 1px solid var(--card-border);
  }

  footer .btn {
    width: 100%;
  }

  .spinner.light {
    border-color: rgba(255, 255, 255, 0.35);
    border-top-color: #fff;
  }
</style>
