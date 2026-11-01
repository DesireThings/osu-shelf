<script lang="ts">
  import { onMount } from "svelte";
  import { getVersion } from "@tauri-apps/api/app";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import {
    loadDownloadOptions,
    removeApiApp,
    saveApiApp,
    saveDownloadOptions,
    savedApiApp,
    thirdPartyLicences,
    type ArchiveValidation,
    type DownloadOptions,
  } from "./api";

  const OSU_APP_PAGE = "https://osu.ppy.sh/home/account/edit#new-oauth-application";

  // ── osu! API app ──────────────────────────────────────────────────────────
  let apiApp = $state<string | null>(null);
  let replacing = $state(false);
  let clientId = $state("");
  let clientSecret = $state("");
  let checking = $state(false);
  let apiError = $state<string | null>(null);
  let apiSaved = $state(false);

  const showAppForm = $derived(apiApp === null || replacing);
  const appReady = $derived(clientId.trim() !== "" && clientSecret.trim() !== "" && !checking);

  async function saveApp() {
    checking = true;
    apiError = null;
    apiSaved = false;
    try {
      apiApp = await saveApiApp(clientId, clientSecret);
      clientId = "";
      clientSecret = "";
      replacing = false;
      apiSaved = true;
    } catch (e) {
      apiError = String(e);
    } finally {
      checking = false;
    }
  }

  async function removeApp() {
    apiError = null;
    apiSaved = false;
    try {
      await removeApiApp();
      apiApp = null;
      replacing = false;
    } catch (e) {
      apiError = String(e);
    }
  }

  // ── mirrors and downloads ─────────────────────────────────────────────────
  let options = $state<DownloadOptions | null>(null);
  /** What's on disk, to tell whether there's anything to save. */
  let savedJson = $state("");
  let loadError = $state<string | null>(null);
  let saving = $state(false);
  let saveError = $state<string | null>(null);
  let justSaved = $state(false);

  const dirty = $derived(options !== null && JSON.stringify(options) !== savedJson);
  const enabledCount = $derived(
    options
      ? options.mirrors.filter((m) => m.enabled).length +
          options.custom_mirrors.filter((u) => u.trim() !== "").length
      : 0,
  );

  const checks: { id: ArchiveValidation; name: string; blurb: string }[] = [
    { id: "magic", name: "Basic (recommended)", blurb: "Checks each file starts like a map archive." },
    { id: "eocd", name: "Strict", blurb: "Also checks the end of each file. Turn it down if many maps fail." },
    { id: "off", name: "Off", blurb: "Only checks the file isn't empty." },
  ];

  function adopt(loaded: DownloadOptions) {
    options = loaded;
    savedJson = JSON.stringify(loaded);
  }

  onMount(async () => {
    try {
      const [app, loaded] = await Promise.all([savedApiApp(), loadDownloadOptions()]);
      apiApp = app;
      adopt(loaded);
    } catch (e) {
      loadError = String(e);
    }
  });

  function move(index: number, by: -1 | 1) {
    if (!options) return;
    const target = index + by;
    if (target < 0 || target >= options.mirrors.length) return;
    const list = options.mirrors;
    [list[index], list[target]] = [list[target], list[index]];
  }

  function setThreads(auto: boolean) {
    if (!options) return;
    options.threads = auto ? null : (options.threads ?? options.default_threads);
  }

  async function save() {
    if (!options) return;
    // An emptied number box reads back as null; the backend wants a number.
    const secs = Math.floor(Number(options.rate_limit_secs));
    options.rate_limit_secs = Number.isFinite(secs) && secs >= 1 ? secs : 60;
    saving = true;
    saveError = null;
    justSaved = false;
    try {
      await saveDownloadOptions(options);
      adopt(await loadDownloadOptions());
      justSaved = true;
    } catch (e) {
      saveError = String(e);
    } finally {
      saving = false;
    }
  }

  function undo() {
    if (savedJson) options = JSON.parse(savedJson);
    saveError = null;
  }

  // Any edit after a save clears the "Saved" note.
  $effect(() => {
    if (dirty) justSaved = false;
  });

  // ── about ─────────────────────────────────────────────────────────────────
  let version = $state("");
  let licences = $state<string | null>(null);
  let showLicences = $state(false);
  let licencesError = $state<string | null>(null);

  onMount(async () => {
    version = await getVersion();
  });

  async function toggleLicences() {
    showLicences = !showLicences;
    if (!showLicences || licences !== null) return;
    licencesError = null;
    try {
      licences = await thirdPartyLicences();
    } catch (e) {
      licencesError = String(e);
      showLicences = false;
    }
  }
</script>

<div class="settings-wrap">
  <header class="page-head">
    <h1>Settings</h1>
    <p class="muted">These apply to the next download. The terminal version of osu-shelf uses the same settings.</p>
  </header>

  {#if loadError}
    <div class="card error">
      <h2>Couldn't load the settings</h2>
      <p>{loadError}</p>
    </div>
  {:else if !options}
    <p class="muted">Loading…</p>
  {:else}
    <div class="settings-grid">
      <div class="settings-col">
        <section class="card">
          <h2>osu! API app</h2>
          <p class="muted small">
            Only needed to search for maps through osu!, which the terminal version can do for now.
            Downloading collections works without it.
          </p>

          {#if apiApp !== null}
            <div class="path-row">
              <span class="ok status-line">Saved: app {apiApp}</span>
              {#if !replacing}
                <button class="secondary" onclick={() => (replacing = true)}>Use a different app</button>
              {/if}
              <button class="secondary" onclick={removeApp}>Remove</button>
            </div>
          {/if}
          {#if apiSaved}
            <p class="ok">osu! accepted the app and it's saved.</p>
          {/if}

          {#if showAppForm}
            <ol class="howto small">
              <li>
                Open your osu! account settings, go to <b>OAuth</b> and press <b>New OAuth Application</b>.
                <button class="link" onclick={() => openUrl(OSU_APP_PAGE)}>Open osu! account settings</button>
              </li>
              <li>Give it any name. Leave the callback URL empty.</li>
              <li>Copy its client ID and client secret into the boxes below.</li>
            </ol>
            <div class="form-grid">
              <label class="field-label" for="client-id">Client ID</label>
              <input
                id="client-id"
                class="text-input"
                inputmode="numeric"
                autocomplete="off"
                spellcheck="false"
                placeholder="A number, e.g. 12345"
                bind:value={clientId}
              />
              <label class="field-label" for="client-secret">Client secret</label>
              <input
                id="client-secret"
                class="text-input"
                type="password"
                autocomplete="off"
                spellcheck="false"
                placeholder="Paste it here"
                bind:value={clientSecret}
              />
            </div>
            <p class="muted small">
              osu-shelf asks osu! whether these work and only keeps them if they do. They're stored
              on this computer, readable only by you.
            </p>
            <div class="actions left">
              <button onclick={saveApp} disabled={!appReady}>
                {checking ? "Checking with osu!…" : "Check and save"}
              </button>
              {#if replacing}
                <button class="secondary" onclick={() => (replacing = false)}>Cancel</button>
              {/if}
            </div>
          {/if}
          {#if apiError}
            <p class="error-text">{apiError}</p>
          {/if}
        </section>

        <section class="card">
          <h2>Downloads</h2>
          <div class="options">
            <label class="option">
              <input type="checkbox" bind:checked={options.video} />
              <span class="text">
                <span class="label">Include background videos</span>
                <span class="hint">Turning this off makes downloads a lot smaller.</span>
              </span>
            </label>
            <label class="option">
              <input type="checkbox" bind:checked={options.skip_already_imported} />
              <span class="text">
                <span class="label">Skip maps you already have in osu!</span>
                <span class="hint">They're still added to the collection.</span>
              </span>
            </label>

            <div class="option">
              <span class="text">
                <span class="label">Maps downloaded at once</span>
                <span class="choice-inline">
                  <label>
                    <input
                      type="radio"
                      name="threads"
                      checked={options.threads === null}
                      onchange={() => setThreads(true)}
                    />
                    Automatic ({options.default_threads} on this computer)
                  </label>
                  <label>
                    <input
                      type="radio"
                      name="threads"
                      checked={options.threads !== null}
                      onchange={() => setThreads(false)}
                    />
                    Choose:
                  </label>
                  <input
                    class="text-input number"
                    type="number"
                    min="1"
                    max="100"
                    aria-label="Maps downloaded at once"
                    disabled={options.threads === null}
                    value={options.threads ?? options.default_threads}
                    oninput={(e) => {
                      const n = Number(e.currentTarget.value);
                      if (options && Number.isInteger(n)) options.threads = n;
                    }}
                  />
                </span>
                {#if options.threads !== null && options.threads > 20}
                  <span class="hint warn-text">Above 20, mirrors are more likely to start refusing downloads for a while.</span>
                {/if}
              </span>
            </div>

            <div class="option">
              <input id="come-back" type="checkbox" bind:checked={options.auto_skip_rate_limited} />
              <span class="text">
                <label class="label" for="come-back">Come back to maps when a mirror is busy</label>
                <span class="hint">
                  After waiting
                  <input
                    class="text-input number inline"
                    type="number"
                    min="1"
                    aria-label="Seconds to wait on a busy mirror"
                    disabled={!options.auto_skip_rate_limited}
                    bind:value={options.rate_limit_secs}
                  />
                  seconds for a busy mirror, a map goes to the back of the queue and is tried again later.
                </span>
              </span>
            </div>

            <div class="option">
              <span class="text">
                <span class="label">Check downloaded files</span>
                <span class="choice-inline">
                  {#each checks as check (check.id)}
                    <label>
                      <input type="radio" name="archive" value={check.id} bind:group={options.archive_validation} />
                      {check.name}
                    </label>
                  {/each}
                </span>
                <span class="hint">
                  {checks.find((c) => c.id === options?.archive_validation)?.blurb}
                </span>
              </span>
            </div>
          </div>
        </section>
      </div>

      <div class="settings-col">
        <section class="card">
          <h2>Mirrors</h2>
          <p class="muted small">
            Maps are downloaded from these sites, top first. When one doesn't have a map or is busy,
            the next one is tried.
          </p>
          <ul class="mirror-list">
            {#each options.mirrors as mirror, i (mirror.host)}
              <li class="mirror">
                <label class="mirror-main">
                  <input type="checkbox" bind:checked={mirror.enabled} />
                  <span class="text">
                    <span class="label">{mirror.name} <span class="muted small">{mirror.host} · {mirror.limits ?? "limits unknown"}</span></span>
                    {#if mirror.note}
                      <span class="hint">{mirror.note}</span>
                    {/if}
                  </span>
                </label>
                <span class="entry-buttons">
                  <button
                    class="secondary icon"
                    aria-label="Move {mirror.name} up"
                    disabled={i === 0}
                    onclick={() => move(i, -1)}>↑</button
                  >
                  <button
                    class="secondary icon"
                    aria-label="Move {mirror.name} down"
                    disabled={i === options.mirrors.length - 1}
                    onclick={() => move(i, 1)}>↓</button
                  >
                </span>
              </li>
            {/each}
          </ul>

          <h3 class="field-label">Your own mirrors</h3>
          <p class="muted small">
            Tried after the ones above. Enter a download link with <code>{"{id}"}</code> where the map
            set number goes, like <code>https://example.com/d/{"{id}"}</code>.
          </p>
          {#each options.custom_mirrors as _, i (i)}
            <div class="add-row custom-row">
              <input
                class="text-input"
                spellcheck="false"
                placeholder={"https://example.com/d/{id}"}
                aria-label="Custom mirror {i + 1}"
                bind:value={options.custom_mirrors[i]}
              />
              <button class="secondary" onclick={() => options?.custom_mirrors.splice(i, 1)}>Remove</button>
            </div>
            {#if options.custom_mirrors[i].trim() !== "" && !options.custom_mirrors[i].includes("{id}")}
              <p class="warn-text small row-warning">
                This link needs <code>{"{id}"}</code> where the map set number goes.
              </p>
            {/if}
          {/each}
          <button class="secondary" onclick={() => options?.custom_mirrors.push("")}>Add a mirror</button>
          {#if enabledCount === 0}
            <p class="warn-text">Turn on at least one mirror, or nothing can be downloaded.</p>
          {/if}
        </section>
      </div>
    </div>

    <div class="actions save-bar">
      {#if saveError}
        <p class="error-text save-error">Couldn't save: {saveError}</p>
      {/if}
      {#if justSaved}
        <span class="ok">Saved.</span>
      {/if}
      <button class="secondary" onclick={undo} disabled={!dirty || saving}>Undo changes</button>
      <button class="primary" onclick={save} disabled={!dirty || saving}>
        {saving ? "Saving…" : "Save changes"}
      </button>
    </div>
  {/if}

  <section class="card about">
    <h2>About</h2>
    <p>osu-shelf {version}</p>
    <p class="muted small">
      Unofficial tool, not made by ppy (osu!) or osu!Collector. Free software under MIT OR
      Apache-2.0; source code at github.com/DesireThings/osu-shelf.
    </p>
    <button class="secondary" onclick={toggleLicences}>
      {showLicences ? "Hide" : "Show"} third-party licences
    </button>
    {#if licencesError}
      <p class="error-text">{licencesError}</p>
    {/if}
    {#if showLicences && licences !== null}
      <pre class="licences">{licences}</pre>
    {/if}
  </section>
</div>

<style>
  .settings-wrap {
    max-width: 1680px;
    margin-inline: auto;
  }
  .settings-grid {
    display: grid;
  }
  .settings-col {
    min-width: 0;
  }
  @container (min-width: 900px) {
    .settings-grid {
      grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
      column-gap: 16px;
      align-items: start;
    }
  }
  .about p {
    max-width: 72ch;
  }
  .status-line {
    flex: 1;
    font-size: 0.95rem;
    font-weight: 600;
  }
  .form-grid {
    display: grid;
    grid-template-columns: max-content minmax(0, 1fr);
    gap: 10px 14px;
    align-items: center;
    margin: 12px 0 4px;
  }
  .mirror-list {
    list-style: none;
    margin: 12px 0 20px;
    padding: 0;
    display: grid;
    gap: 6px;
  }
  .mirror {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 8px 10px;
    border: 1px solid var(--border);
    border-radius: 8px;
  }
  .mirror-main,
  .option {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    min-width: 0;
  }
  .mirror-main input,
  .option > input {
    margin-top: 4px;
  }
  button.icon {
    padding: 4px 10px;
  }
  .custom-row {
    margin-bottom: 8px;
  }
  .options {
    display: grid;
    gap: 16px;
  }
  .choice-inline {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 16px;
    align-items: center;
    margin: 4px 0;
  }
  .text-input.number {
    flex: none;
    width: 6em;
    padding: 6px 8px;
  }
  .text-input.inline {
    width: 5em;
    margin: 0 4px;
  }
  h3.field-label {
    margin: 0 0 4px;
  }
  .row-warning {
    margin: -4px 0 8px;
  }
  .save-error {
    flex: 1;
    margin: 0;
  }
  .save-bar {
    position: sticky;
    bottom: 0;
    gap: 10px;
    align-items: center;
    padding: 12px 0;
    background: var(--bg);
  }
  .licences {
    margin: 12px 0 0;
    max-height: 420px;
    overflow: auto;
    padding: 12px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--bg);
    font-size: 0.78rem;
    white-space: pre-wrap;
  }
  @media (max-width: 560px) {
    .form-grid {
      grid-template-columns: 1fr;
    }
  }
</style>
