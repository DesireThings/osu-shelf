<script lang="ts">
  import { onMount } from "svelte";
  import Setup from "./lib/Setup.svelte";
  import Collections from "./lib/Collections.svelte";
  import Import from "./lib/Import.svelte";
  import SettingsPage from "./lib/Settings.svelte";
  import Update from "./lib/Update.svelte";
  import Find from "./lib/Find.svelte";
  import {
    detectInstalls,
    loadSettings,
    type Installs,
    type RunKind,
    type Settings,
  } from "./lib/api";
  import { initDownloads, store } from "./lib/store.svelte";

  type Page = "setup" | "collections" | "import" | "find" | "update" | "settings";

  const steps: { id: Page; label: string; hint: string }[] = [
    { id: "setup", label: "Set up", hint: "Your osu! and a download folder" },
    { id: "collections", label: "Add collections", hint: "Paste osu!collector links" },
    { id: "import", label: "Import into osu!", hint: "Get everything into the game" },
  ];

  let page = $state<Page>("setup");
  let settings = $state<Settings | null>(null);
  let installs = $state<Installs | null>(null);
  let loadError = $state<string | null>(null);

  /** Sidebar hint while a run of `kind` downloads, so it shows from any page. */
  function downloadingHint(kind: RunKind) {
    const running = Object.values(store.downloads).find(
      (s) => s.kind === kind && (s.phase === "downloading" || s.phase === "preparing"),
    );
    if (!running) return null;
    if (running.phase === "preparing") return `Getting ready: ${running.name}`;
    const settled = running.downloaded + running.skipped + running.failed;
    return `Downloading ${settled}/${running.total}: ${running.name}`;
  }
  const collectionsHint = $derived(downloadingHint("collection"));
  const findHint = $derived(
    store.find.searching ? "Searching…" : downloadingHint("find"),
  );
  const updateHint = $derived(
    store.updates.scanning ? "Checking for new maps…" : downloadingHint("update"),
  );

  onMount(async () => {
    try {
      [settings, installs] = await Promise.all([
        loadSettings(),
        detectInstalls(),
        initDownloads(),
      ]);
    } catch (e) {
      loadError = String(e);
    }
  });
</script>

<div class="shell">
  <nav class="sidebar">
    <div class="brand">osu-<span>shelf</span></div>
    <ol class="steps">
      {#each steps as step, i (step.id)}
        <li>
          <button
            class="step"
            class:active={page === step.id}
            onclick={() => (page = step.id)}
          >
            <span class="num">{i + 1}</span>
            <span class="text">
              <span class="label">{step.label}</span>
              {#if step.id === "collections" && collectionsHint}
                <span class="hint live">{collectionsHint}</span>
              {:else}
                <span class="hint">{step.hint}</span>
              {/if}
            </span>
          </button>
        </li>
      {/each}
    </ol>
    <p class="group-label">More ways to get maps</p>
    <button class="step" class:active={page === "find"} onclick={() => (page = "find")}>
      <span class="num" aria-hidden="true">⌕</span>
      <span class="text">
        <span class="label">Find maps</span>
        {#if findHint}
          <span class="hint live">{findHint}</span>
        {:else}
          <span class="hint">Search by stars, mapper and more</span>
        {/if}
      </span>
    </button>
    <button class="step" class:active={page === "update"} onclick={() => (page = "update")}>
      <span class="num" aria-hidden="true">↻</span>
      <span class="text">
        <span class="label">Update collections</span>
        {#if updateHint}
          <span class="hint live">{updateHint}</span>
        {:else}
          <span class="hint">Get maps added since you downloaded</span>
        {/if}
      </span>
    </button>
    <button
      class="step settings-link"
      class:active={page === "settings"}
      onclick={() => (page = "settings")}
    >
      <span class="num" aria-hidden="true">⚙</span>
      <span class="text">
        <span class="label">Settings</span>
        <span class="hint">Mirrors, downloads, osu! API app</span>
      </span>
    </button>
    <p class="unofficial">Unofficial tool, not made by ppy (osu!) or osu!Collector.</p>
  </nav>

  <main class="content">
    <div class="page" class:page-wide={page === "find" || page === "settings"}>
      {#if loadError}
        <div class="card error">
          <h2>Couldn't start</h2>
          <p>{loadError}</p>
        </div>
      {:else if !settings || !installs}
        <p class="muted">Loading…</p>
      {:else if page === "setup"}
        <Setup bind:settings {installs} onDone={() => (page = "collections")} />
      {:else if page === "collections"}
        <Collections onNext={() => (page = "import")} />
      {:else if page === "import"}
        <Import {settings} />
      {:else if page === "find"}
        <Find onSettings={() => (page = "settings")} onImport={() => (page = "import")} />
      {:else if page === "update"}
        <Update {settings} onImport={() => (page = "import")} />
      {:else}
        <SettingsPage />
      {/if}
    </div>
  </main>
</div>
