<script lang="ts">
	import type { Post } from '$lib/types/Post';
	import { createWindowVirtualizer } from '@tanstack/svelte-virtual';

	import { Post as PostComponent } from './post';

	let { posts, onendreached }: { posts: Array<Post>; onendreached?: () => void } = $props();

	let timelineElement: HTMLDivElement | undefined = $state();

	let virtualizer = $derived(
		createWindowVirtualizer<HTMLDivElement>({
			count: posts.length,
			scrollMargin: timelineElement?.offsetTop ?? 0,
			// good enough guess
			estimateSize: () => 100,
			// give a big enough buffer for us to load more posts
			overscan: 5
		})
	);

	let virtualItems = $derived($virtualizer.getVirtualItems());
	let virtualElements: HTMLDivElement[] = $state([]);

	// Recompute the size of the virtual list.
	// Otherwise we can't possibly correctly reach the bottom of the list.
	$effect(() => {
		virtualElements.forEach((element) => $virtualizer.measureElement(element));
	});

	// Emit event when we reached the end.
	// The callee has to debounce the events themselves.
	$effect(() => {
		if (virtualElements.length === 0) return;

		let [lastItem] = virtualItems.toReversed();
		if (lastItem.index === posts.length - 1) {
			if (onendreached) onendreached();
		}
	});
</script>

<div bind:this={timelineElement} style="height: {$virtualizer.getTotalSize()}px;">
	<div
		style="transform: translateY({virtualItems[0]
			? virtualItems[0].start - $virtualizer.options.scrollMargin
			: 0}px);"
	>
		{#each virtualItems as row (posts[row.index].id)}
			<div bind:this={virtualElements[row.index]} data-index={row.index}>
				<PostComponent primary={false} {...posts[row.index]} />
				<div class="divider"></div>
			</div>
		{/each}
	</div>
</div>
