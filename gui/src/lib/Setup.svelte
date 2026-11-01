<script lang="ts">
  import { open } from "@tauri-apps/plugin-dialog";
  import { saveSettings, type Installs, type OsuClient, type Settings } from "./api";

  let {
    settings = $bindable(),
    installs,
    onDone,
  }: { settings: Settings; installs: Installs; onDone: () => void } = $props();

  let saving = $state(false);
  let error = $state<string | null>(null);

  const clients: { id: OsuClient; name: string; blurb: string }[] = [
    { id: "lazer", name: "osu!lazer", blurb: "The new osu! (also Torii and other lazer builds)" },
    { id: "stable", name: "osu! stable", blurb: "The classic osu! client" },
  ];

  const detected = $derived(
    settings.client === "lazer" ? (installs.lazer[0] ?? null) : installs.stable,
  );
  const osuFolder = $derived(settings.osu_path ?? detected);

  function pickClient(client: OsuClient) {
    if (settings.client !== client) {
      settings.client = client;
      // A folder picked for the other client doesn't apply to this one.
      settings.osu_path = null;
    }
  }

  async function browse(title: string): Promise<string | null> {
    const picked = await open({ directory: true, multiple: false, title });
    return typeof picked === "string" ? picked : null;
  }

  async function chooseOsuFolder() {
    const label = settings.client === "lazer" ? "lazer's data folder" : "your osu! folder";
    const picked = await browse(`Choose ${label}`);
    if (picked) settings.osu_path = picked;
  }

  async function chooseDownloadFolder() {
    const picked = await browse("Choose where downloaded maps go");
    if (picked) settings.download_dir = picked;
  }

  async function save() {
    saving = true;
    error = null;
    try {
      await saveSettings(settings);
      onDone();
    } catch (e) {
      error = String(e);
    } finally {
      saving = false;
    }
  }
</script>

<header class="page-head">
  <h1>Set up</h1>
  <p class="muted">Two quick choices. You can change them any time.</p>
</header>

<section class="card">
  <h2>Which osu! do you play?</h2>
  <div class="choice-row">
    {#each clients as client (client.id)}
      <button
        class="choice"
        class:selected={settings.client === client.id}
        onclick={() => pickClient(client.id)}
      >
        <span class="choice-name">{client.name}</span>
        <span class="choice-blurb">{client.blurb}</span>
      </button>
    {/each}
  </div>

  <div class="field">
    <span class="field-label">
      {settings.client === "lazer" ? "lazer data folder" : "osu! folder"}
    </span>
    {#if osuFolder}
      <div class="path-row">
        <code class="path">{osuFolder}</code>
        <button class="secondary" onclick={chooseOsuFolder}>Change…</button>
      </div>
      <p class="ok">
        {settings.osu_path ? "Using the folder you picked." : "Found automatically."}
      </p>
    {:else}
      <div class="path-row">
        <span class="muted">Not found automatically.</span>
        <button onclick={chooseOsuFolder}>Choose folder…</button>
      </div>
      <p class="muted small">
        {settings.client === "lazer"
          ? "It's the folder with client.realm in it. In osu!lazer: Settings → Maintenance → Open osu! folder."
          : "It's the folder with osu!.exe and osu!.db in it."}
      </p>
    {/if}
    {#if settings.client === "lazer" && installs.lazer.length > 1}
      <p class="detected-label">Detected lazer installs</p>
      <div class="installs">
        {#each installs.lazer as path (path)}
          <label class="install" class:selected={osuFolder === path}>
            <input
              type="radio"
              name="lazer-install"
              checked={osuFolder === path}
              onchange={() => (settings.osu_path = path === installs.lazer[0] ? null : path)}
            />
            <code class="path">{path}</code>
            {#if path === installs.lazer[0]}<span class="muted small">newest</span>{/if}
          </label>
        {/each}
      </div>
      <p class="muted small">
        The newest install is used unless you pick another, so separate osu! and Torii builds can
        keep their own folders.
      </p>
    {/if}
  </div>
</section>

<section class="card">
  <h2>Where should downloaded maps go?</h2>
  <div class="field">
    <span class="field-label">Download folder</span>
    <div class="path-row">
      <code class="path">{settings.download_dir}</code>
      <button class="secondary" onclick={chooseDownloadFolder}>Change…</button>
    </div>
    <p class="muted small">Each collection gets its own folder in here.</p>
  </div>
</section>

{#if error}
  <p class="error-text">Couldn't save: {error}</p>
{/if}

<div class="actions">
  <button class="primary" onclick={save} disabled={saving}>
    {saving ? "Saving…" : "Save and continue"}
  </button>
</div>

<style>
  .detected-label {
    margin-top: 10px;
  }
  .installs {
    display: grid;
    gap: 8px;
  }
  .install {
    display: flex;
    gap: 10px;
    align-items: center;
    padding: 8px 10px;
    background: var(--surface-2);
    border: 1px solid var(--border);
    border-radius: 8px;
  }
  .install.selected {
    border-color: var(--accent);
  }
  .install .path {
    flex: 1;
  }
</style>
