<script lang="ts">
	import { goto } from '$app/navigation';
	import Logo from '$assets/Logo.svelte';
	import { GQL_RegisterUser } from '$houdini';
	import Hero from '$lib/components/Hero.svelte';
	import RegisterForm from '$lib/components/RegisterForm.svelte';
	import { Button } from '$lib/components/input';
	import { loadOAuthApp } from '$lib/oauth/client';
	import { tokenStore } from '$lib/oauth/token';
	import { registerSchema } from '$lib/schemas/register';

	import type { PageData } from './$houdini';
	import IconErrorOutline from '~icons/mdi/error-outline';

	const { data }: { data: PageData } = $props();

	const statsStore = $derived(data.LoadStatistics);
	const stats = $derived({
		characterLimit: $statsStore.data?.instance.characterLimit ?? 0,
		description: $statsStore.data?.instance.description ?? '',
		postCount: $statsStore.data?.instance.localPostCount ?? 0,
		registeredUsers: $statsStore.data?.instance.userCount ?? 0,
		registrationsOpen: $statsStore.data?.instance.registrationsOpen ?? true
	});

	let registerButtonDisabled = $state(false);
	let registerErrors: string[] = $state([]);

	async function doRegister(event: SubmitEvent & { currentTarget: EventTarget & HTMLFormElement }) {
		registerButtonDisabled = true;

		const formData = new FormData(event.currentTarget);
		const validatedData = await registerSchema.safeParseAsync(
			Object.fromEntries(formData.entries())
		);

		if (!validatedData.success) {
			const formattedErrors = validatedData.error.format(
				(issue) => `${issue.path.join(', ')}: ${issue.message}`
			);

			registerErrors = Object.values(formattedErrors).flatMap((error) =>
				'_errors' in error ? error._errors : error
			);
			registerButtonDisabled = false;

			return;
		}

		try {
			const result = await GQL_RegisterUser.mutate(validatedData.data);
			if (result.errors) {
				registerErrors = result.errors.map((error) => error.message);
			} else {
				initiateLogin();
			}
		} catch (reason: unknown) {
			if (reason instanceof Error) {
				registerErrors = [reason.message];
			}
		} finally {
			registerButtonDisabled = false;
		}
	}

	let loginInProcess = $state(false);

	async function initiateLogin() {
		loginInProcess = true;

		const oauthApp = await loadOAuthApp();
		const oauthUrl = new URL(`${window.location.origin}/oauth/authorize`);

		oauthUrl.searchParams.set('response_type', 'code');
		oauthUrl.searchParams.set('client_id', oauthApp.id);
		oauthUrl.searchParams.set('redirect_uri', oauthApp.redirectUri);
		oauthUrl.searchParams.set('scope', 'read write follow');

		window.location.assign(oauthUrl);
	}

	tokenStore.subscribe((newToken) => {
		if (!newToken) return;
		goto('/timeline/home');
	});
</script>

<Hero class="w-full flex-col justify-between lg:flex-row">
	<div class="flex flex-col max-lg:items-center max-lg:text-center">
		<Logo class="max-w-3/4" />

		<h1>Federated microblogging.</h1>

		<p>
			<!-- eslint-disable-next-line svelte/no-at-html-tags -->
			{@html stats.description}
		</p>
	</div>

	<div class="flex flex-col gap-3">
		<div class="bg-base-100 stats max-sm:stats-vertical shadow">
			<div class="stat place-items-center">
				<div class="stat-title">Registered Users</div>
				<div class="stat-value">
					{stats.registeredUsers}
				</div>
			</div>

			<div class="stat place-items-center">
				<div class="stat-title">Authored posts</div>
				<div class="stat-value">
					{stats.postCount}
				</div>
			</div>

			<div class="stat place-items-center">
				<div class="stat-title">Character limit</div>
				<div class="stat-value">
					{stats.characterLimit}
				</div>
			</div>
		</div>

		<div class="card bg-base-100 p-10 shadow-2xl">
			{#if stats.registrationsOpen}
				{#if registerErrors.length !== 0}
					<div role="alert" class="alert alert-error mb-5">
						<IconErrorOutline class="opacity-70" />
						<ol class="list-none p-0">
							{#each registerErrors as error, index (index)}
								<li>{error}</li>
							{/each}
						</ol>
					</div>
				{/if}

				<RegisterForm onregister={doRegister} processing={registerButtonDisabled} />
				<div class="divider">OR</div>
			{/if}

			<Button class="w-full" buttonType="neutral" onclick={initiateLogin} loading={loginInProcess}>
				Already have an account? Sign in
			</Button>
		</div>
	</div>
</Hero>
