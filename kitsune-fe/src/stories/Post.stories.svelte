<script lang="ts" module>
	import { Post as PostComponent } from '$lib/components/post';
	import { defineMeta } from '@storybook/addon-svelte-csf';

	import exampleAvatar from './assets/profile_pic.png';

	const { Story } = defineMeta({
		title: 'Post',
		component: PostComponent,
		tags: ['autodocs']
	});
</script>

<Story
	name="Example"
	args={{
		content: 'This is a test content',
		createdAt: new Date(),
		user: {
			id: 'owo123',
			name: 'Example User',
			username: '@example@kitsune.example',
			avatarUrl: exampleAvatar
		}
	}}
/>
