# Writing Style Guide (Blog & Documentation)

Style rules for Bundlebase blog posts and end-user documentation -- developer-to-developer, honest, unpolished. Blog posts skew personal and casual; docs skew terse and reference-style. Both share the voice rules and the "avoiding AI tropes" section below.

## Voice

- **First person singular** ("I") for personal work, opinions, and decisions. "We" for project-level statements.
- **Developer-to-developer.** Write like you're posting on a technical forum, not issuing a press release.
- **Plainspoken.** No buzzwords, no hype adjectives ("revolutionary", "game-changing", "excited to announce"). Say what the thing does.
- **Comfortable with imperfection.** Admit missed milestones, known issues, and half-baked ideas. "This is not yet production ready" is fine.
- **Honest about uncertainty.** "I'm not sure if this is the right approach" is better than false confidence.

## Structure by Post Type

### Release announcements
- One or two sentence intro: what version, what's notable
- Bulleted list of changes -- no paragraph-per-feature bloat
- Bug-fix-only releases get 2-3 sentences total. Do not pad them

### Roadmap / plans
- State the goal plainly
- Numbered or bulleted plan
- Flag what's uncertain or might change

### Technical deep-dives
- Get to the point in the first sentence
- Use code examples over prose explanations
- Structure with headers and lists, not long paragraphs

### General announcements
- Short. Say the thing, provide a link if relevant, done.

## Do

- Get to the point immediately. One sentence intros, not three paragraphs of context.
- Use bulleted lists for features and changes
- Thank contributors by name
- Admit when something isn't done: "I was hoping to add X, but wanted to get this out first"
- Use casual phrasing: "so get thinking", "hope to see you there", "worst case..."
- Self-deprecating humor when it fits: "The major improvement is that it actually runs now"
- Close with a casual invitation: "Let me know if you run into any issues" or similar

## Don't

- Use marketing language or sales pitch tone
- Use superlatives: "amazing", "incredible", "excited to announce"
- Pad short announcements into long posts
- Write corporate speak: "We are pleased to inform you", "leveraging synergies"
- Over-polish. It should read like it was written quickly and honestly, not workshopped by a comms team.
- Use "stay tuned" or other empty filler closings
- Add sections just to make a post look longer

## Avoiding AI Tropes

Writing that reads as AI-generated erodes trust. One trope occasionally is fine; clusters of them, or any one pattern repeated, gives the game away. Aim for varied, specific, imperfect human prose.

### Word choice
- Drop magic adverbs that manufacture importance: "quietly", "deeply", "fundamentally", "remarkably", "arguably".
- Avoid statistical AI markers: "delve", "utilize", "leverage" (verb), "robust", "streamline", "harness", "seamless".
- Skip grandiose nouns: "tapestry", "landscape", "paradigm", "synergy", "ecosystem". Say the actual thing.
- Use plain copulas. "X is Y", not "X serves as Y" / "stands as" / "represents".

### Sentence structure
- No negative parallelism: "It's not X -- it's Y", "not because X, but because Y", "The question isn't X. The question is Y".
- No dramatic countdowns: "Not X. Not Y. Just Z."
- No self-answered rhetorical questions: "The result? Devastating."
- Don't open multiple sentences identically ("They assume... They assume...").
- Use rule-of-three sparingly. Stacked tricolons read as AI.
- Cut filler transitions: "It's worth noting", "Importantly", "Notably", "Interestingly".
- Cut hollow `-ing` tails that add nothing: "highlighting its importance", "reflecting broader trends".
- Avoid fake "from X to Y" ranges when X and Y aren't on a real spectrum.

### Tone
- No false suspense: "Here's the kicker", "Here's the thing", "Here's where it gets interesting".
- No patronizing analogies: "Think of it as...", "It's like a..." -- unless the analogy is actually load-bearing.
- No "imagine a world where..." futurism.
- Don't assert something is "clear", "simple", or "obvious" without showing it.
- Don't inflate stakes. Most features are not world-historical.
- Cut "Let's break this down" / "Let's unpack" / "Let's explore".
- Name sources. Not "experts argue" or "observers note".
- Don't coin compound labels ("the supervision paradox") without defining and earning them.

### Formatting
- Em dashes: a few per piece, not twenty. When in doubt use `--` or a comma.
- Don't start every bullet with a bolded keyword followed by a colon. Mix it up.
- Straight quotes and ASCII arrows (`->`), not smart quotes or `→`.

### Composition
- Don't announce structure ("In this section we'll explore...") and don't recap it ("As we've seen...").
- Don't repeat the same metaphor across the piece.
- Don't rapid-fire historical analogies ("Apple didn't build Uber. Facebook didn't build Spotify...").
- Don't restate one idea ten different ways.
- Don't end with "In conclusion" / "To sum up" / "In summary".
- Don't use the "despite its challenges, X remains promising" formula.

## Closing Conventions

End with one of:
- A casual call to action: "Let me know if you have questions or run into issues"
- A specific ask: "Try it out and tell me if the new filter syntax makes sense"
- Nothing -- short posts don't need a closing
