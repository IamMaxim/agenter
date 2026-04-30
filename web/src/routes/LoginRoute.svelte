<script lang="ts">
  import { ApiError } from '../api/http';
  import { getCurrentUser, loginPassword } from '../api/auth';
  import type { AuthenticatedUser } from '../api/types';

  export let onLogin: (user: AuthenticatedUser) => void;

  let email = '';
  let password = '';
  let busy = false;
  let error = '';

  async function submit() {
    busy = true;
    error = '';

    try {
      await loginPassword({ email, password });
      onLogin(await getCurrentUser());
    } catch (err) {
      if (err instanceof ApiError && err.status === 401) {
        error = 'Email or password was rejected.';
      } else {
        error = 'Login failed. Check the control plane and try again.';
      }
    } finally {
      busy = false;
    }
  }
</script>

<section class="auth-screen">
  <form class="login-panel" on:submit|preventDefault={submit}>
    <div>
      <p class="eyebrow">Agenter</p>
      <h1>Sign in</h1>
    </div>

    <label>
      Email
      <input type="email" bind:value={email} autocomplete="username" required />
    </label>

    <label>
      Password
      <input type="password" bind:value={password} autocomplete="current-password" required />
    </label>

    {#if error}
      <p class="error" role="alert">{error}</p>
    {/if}

    <button type="submit" disabled={busy}>{busy ? 'Signing in...' : 'Sign in'}</button>
  </form>
</section>
