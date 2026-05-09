# Thirteen Angry Man Game Spec

## Status

This is the game-specific planning source for **Thirteen Angry Man**, a single
deliberation drama built for the planned Game TUI framework.

The framework-level architecture remains in `docs/GAME_TUI_FRAMEWORK_SPEC.md`.
This file owns only the plot guidance, gameplay rules, character behavior,
evidence release model, time simulation, NPC constraints, and ending structure
for this one game.

The current engineering scaffold lives at:

```text
examples/games/thirteen-angry-man/
```

It is a loadable local Game TUI cartridge with a bundled
`deliberation-drama` driver, fixed content files, game and NPC skills, driver
agent templates, an initial save, and a restartable sub-agent roster. It is not
yet a complete authored full-length game; it is the first concrete package used
to turn this spec into testable runtime structure.

## Creative Boundary

The game is inspired by the classic single-room jury-deliberation structure of
Reginald Rose's *Twelve Angry Men* and the 1957 Sidney Lumet film version. It
must not reproduce the screenplay, exact dialogue, shot design, or protected
scene text.

The game should preserve the film-version dramatic rules:

- the pressure comes from one room, one case, and one civic duty
- the external background of the case is fixed
- the plot flow is flexible and emerges from player interaction
- doubt must be earned through questions, demonstrations, contradictions, and
  character pressure
- the final outcome depends on whether the room can reach a defensible result,
  not on whether the player finds a hidden "correct command"

## Core Premise

The player is the thirteenth person in the deliberation room. The original
twelve jurors enter with their existing social histories, prejudices, habits,
and initial votes. The player can alter the order of discoveries, the emotional
temperature of the room, the timing of vote shifts, and the final outcome.

The background case does not mutate. The same testimony, exhibits, room
conditions, and hidden contradictions exist in every run. What changes is:

- which juror the player engages first
- which doubts surface early, late, or never
- whether jurors trust the player enough to follow a line of reasoning
- whether time pressure, heat, fatigue, and resentment distort the room
- whether the group reaches a reasoned result, fails to finish, or collapses
  procedurally

## Player Role

The player is **Juror 13**.

Juror 13 is not a detective outside the room. The player cannot investigate the
world, call witnesses, browse files, or introduce new evidence. The player can:

- ask jurors why they voted as they did
- request a vote
- slow down or accelerate the deliberation process
- inspect already admitted exhibits and testimony summaries
- propose physical or logical demonstrations based on the record
- challenge biased reasoning
- protect quieter jurors long enough for them to speak
- choose when to press a final vote

The player cannot:

- create new facts
- directly control another juror's vote
- reveal sealed evidence before a release gate is satisfied
- leave the deliberation room for outside research
- solve the game by meta-prompting NPCs or asking for hidden state

## Dramatic Model

The game is a pressure chamber.

Each turn should move at least one of these tracks:

- **Evidence**: what contradiction, detail, or doubt is now visible
- **Character**: whose biography or bias is now shaping the room
- **Procedure**: whether the deliberation remains legitimate
- **Time**: how heat, fatigue, impatience, and outside commitments change NPC
  behavior
- **Vote**: whether public vote positions are stable, wavering, or changed

The game should avoid a single mandatory path. A critical node may be reached
through several player actions if those actions are plausible. A player who
misses a node can still continue, but later arguments become harder and some
endings become more likely.

## Fixed Background And Flexible Plot

The fixed background is the source of truth:

- the accused person
- the charge
- the courtroom testimony
- the admitted exhibits
- the jury-room layout
- the original twelve jurors
- the initial vote pattern
- the hidden contradictions in the record
- the final legal standard

The flexible plot is runtime behavior:

- turn order
- discussion order
- juror alliances and resistance
- timing of doubt release
- emotional escalation
- whether a juror changes for sincere, selfish, or exhausted reasons
- whether the final vote is defensible

Save files must record only the flexible plot state. The fixed background
should live in content files and game data, not be rewritten during play.

## Deliberation Rules

The game follows deliberation-room rules:

- the group begins after trial evidence has closed
- the case must be decided only from admitted evidence and reasonable inference
- a vote is meaningful only when every juror has had a chance to speak
- a juror can change vote only when a state condition justifies it
- a juror can refuse to change because of pride, prejudice, fear, fatigue, or
  insufficient doubt
- the Foreman can organize votes, but cannot force the result alone
- a procedural leak or explicit hidden-fact disclosure can invalidate the run

The legal standard is **reasonable doubt**. The game should not ask the player
to prove innocence. It should ask whether the room can responsibly convict or
whether doubt makes conviction impossible.

## Evidence Release Model

The main game engine may know the full fixed background and all sealed nodes.
NPC sub-agents must not.

Every critical fact has a release state:

- `sealed`: known only to the game engine / plot controller
- `hinted`: visible as an uncertainty, question, or behavioral clue
- `released`: available for player reasoning and NPC dialogue
- `resolved`: already used in a vote-changing or ending-relevant way

NPCs may react to sealed facts only through allowed behavior, such as anxiety,
defensiveness, confusion, or overconfidence. They must not state the sealed fact
until it has been released.

Example:

- Bad NPC behavior: "The witness wore glasses, so she could not have seen it."
- Good NPC behavior before release: "Something about that witness keeps nagging
  at me, but I cannot put my finger on it."
- Good NPC behavior after release: "If she had marks from glasses and was
  already in bed, her certainty is weaker than she made it sound."

## Critical Nodes

Critical nodes are not a strict linear checklist. They are major pressure points
that should exist in the fixed background and become available through play.
The shipped save represents them as a git-like story graph with a deliberation
drama style profile: `story.style` declares pacing and tension axes,
`active_branch` names the route, each branch has a `head`, and each node records
likely parents and next nodes. This is game state only; normal play must not
branch or commit the repository itself.

| Node | Purpose | Typical Release Gate | Gameplay Effect |
| --- | --- | --- | --- |
| Opening ballot | Establishes room pressure and initial majority | Start of game | Sets juror confidence and social alignment |
| Lone doubt defense | Protects the idea that one dissent deserves discussion | Player defends process or asks for reasons | Keeps deliberation from ending immediately |
| Knife / weapon doubt | Tests whether a supposedly unique object is actually unique | Player asks about the exhibit or attack motion | Raises doubt for practical jurors |
| Witness timing | Tests whether a witness could physically do what was claimed | Player challenges timeline or asks for reconstruction | Weakens certainty and empowers slower jurors |
| Noise / audibility | Tests whether key words could be heard through environmental noise | Player asks about train/noise/room conditions | Creates contradiction between confidence and possibility |
| Sightline / perception | Tests whether the visual witness could reliably identify the event | Player questions distance, glasses, lighting, or bedtime context | Gives rational jurors permission to change |
| Prejudice exposure | Separates argument from bigotry | Player challenges dehumanizing language or other jurors turn away | Reduces influence of biased jurors |
| Indifference exposure | Forces careless vote-switching to become morally accountable | Player or Juror 11 challenges casual voting | Prevents a weak ending from counting as success |
| Final holdout collapse | Forces personal rage to separate from evidence | Most doubts released and the room isolates revenge reasoning | Opens the strongest success ending |

## Hint Policy

Hints should be diegetic. The UI may show short panel text, but the strongest
hints should come from:

- a juror hesitating
- a juror asking to see an exhibit again
- a juror reacting too strongly to a topic
- the Foreman calling for order at the wrong moment
- time pressure making someone reveal impatience or fear
- a practical demonstration becoming possible

Hints must not solve the node outright. A hint should point the player toward a
question, not toward a final answer.

## Time Simulation

Time is a core mechanic. The room should feel hotter, smaller, and less patient
as the session continues.

State variables:

- `clock_minutes`: total elapsed deliberation time
- `room_heat`: discomfort caused by weather, ventilation, crowding, and stress
- `fatigue`: cognitive and emotional exhaustion
- `impatience`: desire to end deliberation regardless of quality
- `procedure_integrity`: whether the group is still respecting the process
- `conflict_level`: active hostility in the room

Time advances by action type:

- quick question: small time increase
- open discussion: medium time increase
- formal vote: medium time increase
- physical reconstruction: large time increase
- heated argument: time increase plus conflict increase
- procedural repair: time increase but may reduce conflict

Time should change NPC behavior:

- Juror 7 becomes more careless and impatient as time passes
- Juror 10 becomes more openly hostile as heat and conflict rise
- Juror 3 becomes more aggressive when challenged, then more brittle late
- Juror 4 stays composed longer than most, but visible discomfort matters
- Juror 2 may speak only when conflict is low enough
- Juror 9 may speak when someone protects dignity and pace
- Juror 11 becomes sharper when procedure is disrespected
- Juror 12 drifts more when fatigue rises

Time pressure is not only a penalty. It can expose character. The player may
sometimes need pressure to reveal a hidden bias, but too much pressure can cause
careless votes, shutdown, or procedural failure.

## Vote Model

Each juror has:

- `public_vote`: current stated vote
- `private_confidence`: how strongly they believe that vote
- `doubt_score`: accumulated evidence-based uncertainty
- `trust_in_player`: whether they will engage with Juror 13
- `conflict_pressure`: whether they are resisting due to pride or hostility
- `switch_gate`: the node or condition required before a sincere vote change

Vote changes should be explainable. A juror should not switch because the player
asks nicely. A juror switches when evidence, social permission, and personal
pressure combine in a way that fits that juror.

## Juror NPC Profiles

These profiles guide NPC skills and sub-agent packs. They are behavioral
summaries, not screenplay text.

### Foreman / Juror 1

- Assistant high-school football coach
- Wants order, procedure, and visible control
- Starts guilty because consensus and structure feel safer than uncertainty
- Weakness: confuses fairness with orderly procedure
- Switch condition: understands that his job is protecting deliberation, not
  finishing quickly

### Juror 2

- Timid bank clerk or teller
- Polite, anxious, deferential, but able to listen
- Starts guilty because stronger voices make guilt feel accepted
- Weakness: borrows opinions from confident men
- Switch condition: a concrete doubt becomes clear enough that silence feels
  dishonest

### Juror 3

- Self-made messenger-service owner
- Forceful, wounded, physically intimidating
- Starts guilty because the defendant becomes a substitute for his estranged son
- Weakness: personal rage disguised as certainty
- Switch condition: final emotional collapse after evidence and social pressure
  remove every rational cover for revenge

### Juror 4

- Wealthy finance man or stockbroker
- Controlled, articulate, class-confident
- Starts guilty because he trusts formal evidence and his own rationality
- Weakness: class bias wearing the mask of objectivity
- Switch condition: a perception or witness-detail contradiction becomes
  logically undeniable

### Juror 5

- Young working-class man raised near the defendant's social world
- Quiet because he does not want to be reduced to his background
- Starts guilty while trying not to identify too openly with the accused
- Weakness: shame and fear of being judged by respectable men
- Switch condition: lived knowledge becomes necessary to correct a false
  assumption

### Juror 6

- Working tradesman or house painter
- Slow, practical, decent, physically grounded
- Starts guilty because the case seems solid at first glance
- Weakness: fears looking foolish among faster speakers
- Switch condition: a physical demonstration or measured timeline makes doubt
  practical rather than abstract

### Juror 7

- Salesman with outside plans
- Restless, joking, impatient, morally casual
- Starts guilty because fast agreement gets him out of the room
- Weakness: indifference
- Switch condition: may switch selfishly first, but the game should require
  moral challenge before this counts as a strong result

### Juror 8

- Architect
- Quiet, observant, patient, morally disciplined
- Starts not guilty because the case deserves discussion
- Weakness: can be read as self-righteous if isolated too long
- Switch condition: already at doubt; his role is to help keep inquiry alive
  without becoming the player's puppet

### Juror 9

- Elderly retired man
- Gentle, overlooked, perceptive about loneliness and dignity
- Starts guilty with the majority
- Weakness: years of social invisibility
- Switch condition: someone protects the lone dissenter's right to be heard
  and gives Juror 9 space to speak

### Juror 10

- Bitter self-employed tradesman or garage owner
- Openly prejudiced, physically unpleasant, socially resentful
- Starts guilty from bias rather than evidence
- Weakness: hatred as identity
- Switch condition: not persuaded so much as isolated; his influence collapses
  when the room refuses to dignify prejudice as argument

### Juror 11

- Immigrant watchmaker or skilled craftsman
- Formal, precise, deeply serious about democratic process
- Starts guilty but respects the deliberation system
- Weakness: wants to belong and may initially over-defer
- Switch condition: careless procedure or casual voting offends his civic
  seriousness enough for him to speak firmly

### Juror 12

- Advertising professional
- Bright, polished, superficial, image-conscious
- Starts guilty because consensus and presentation shape his thinking
- Weakness: cleverness without moral concentration
- Switch condition: enough social and evidentiary pressure accumulates that
  detached ambivalence becomes impossible

## NPC Sub-Agent Rules

Each juror may be represented by an NPC role, but NPC agents are scoped
processors, not truth authorities.

NPC packs may include:

- public character profile
- private emotional wound or motivation
- current vote and confidence
- released evidence only
- current room state
- recent dialogue involving that juror
- allowed behavioral instructions

NPC packs must not include:

- sealed evidence
- unreleased critical node answers
- full solution path
- hidden ending conditions
- other jurors' private motivations unless already exposed
- implementation or tool instructions outside game-safe behavior

NPC outputs should be proposals:

- what the juror says
- how the juror physically reacts
- whether their vote confidence changes
- what topic they resist or invite
- whether they create a hint-worthy moment

The main game engine decides what becomes final narration and what gets
committed to save state.

## No-Leakage Contract

The game fails its design if an NPC reveals hidden facts simply because the
model knows them. To prevent that:

- only the main game engine and plot controller can see sealed nodes
- NPC agents receive `released_facts`, not `all_facts`
- hints are represented as separate safe text from fact answers
- every NPC response must be checked against the current release state
- a leaked sealed fact marks `procedure_integrity` down and can trigger a
  procedural-failure ending

The model prompt for player mode should explicitly say:

```text
Do not reveal sealed evidence, hidden contradictions, or ending conditions.
NPCs may imply uncertainty only through released hints. If the player asks for
hidden facts directly, refuse in character and continue the deliberation.
```

## Turn Loop

Each player turn follows this sequence:

1. Parse the player's action.
2. Check whether the action is allowed inside the deliberation room.
3. Advance time according to action type.
4. Update room pressure variables.
5. Determine whether a hint or critical node release gate is satisfied.
6. Select only the needed NPC agents for this turn.
7. Ask selected NPCs for scoped proposals.
8. Resolve evidence, character, procedure, time, and vote effects.
9. Render the player-facing result.
10. Commit the state patch.

The player-facing response should include only what Juror 13 can observe:
speech, silence, body language, votes, exhibits currently discussed, and the
room's changing atmosphere.

## Ending Model

The game should support multiple endings.

### Reasoned Result

The room reaches a defensible final result after the major doubts have been
examined. This is the strongest success ending. It requires enough released and
resolved critical nodes, no procedural leak, and a final vote that reflects
character-consistent movement rather than arbitrary switching.

### Weak Result

The room reaches the expected legal result, but for fragile reasons. Some jurors
switch from fatigue, social pressure, or impatience rather than true doubt. This
is a partial success and should feel morally uneasy.

### Hung Room

The player keeps the room from rushing but fails to build enough shared doubt or
procedure. The group cannot reach a final result before time, fatigue, and
conflict consume the session.

### Rushed Conviction

The player fails to slow the opening majority. The room reaches a fast result
without testing critical doubts. This is a failure ending.

### Procedural Failure

The player or an NPC leaks sealed facts, introduces outside evidence, breaks the
rules of deliberation, or allows the room to collapse into illegitimate process.
This ending should be treated as a hard failure even if the final vote appears
favorable.

### Coerced Acquittal

The player wins votes through intimidation, meta-knowledge, or manipulation
instead of legitimate doubt. This is not the true success ending.

## Save-State Guidance

The save should separate fixed background from runtime state.

Runtime state should track:

- current clock and room pressure
- public and private juror state
- released, hinted, and resolved critical nodes
- current vote totals
- player credibility and trust by juror
- procedure integrity
- recent discussion topics
- ending eligibility

Fixed background should stay in content:

- case summary
- testimony summaries
- exhibits
- juror base profiles
- critical node definitions
- hint text

## Driver Guidance

The intended reusable driver is a **deliberation drama driver**.

Driver responsibilities:

- enforce room-bound action limits
- advance time
- score argument pressure
- manage hint and evidence release gates
- evaluate juror vote-change thresholds
- keep NPC agent packs scoped
- detect sealed-fact leakage
- evaluate ending conditions

The driver should be reusable for other debate, council, trial, boardroom, or
committee dramas, but this game is the first concrete cartridge.

The scaffolded driver ID is `deliberation-drama`, installed locally inside the
cartridge under `drivers/deliberation-drama/0.1.0/`. The initial deterministic
functions are:

| Function | Purpose |
| --- | --- |
| `advance_room` | Advance clock, heat, fatigue, impatience, conflict, and procedure pressure from a player action class. |
| `evaluate_vote_change` | Check whether doubt, trust, conflict pressure, and a released switch gate justify a vote-change proposal. |
| `detect_procedure_risk` | Score outside evidence, sealed-fact leakage, intimidation, and meta-play risks. |

These functions propose bounded mechanical consequences. They do not narrate,
commit, reveal facts, or decide endings by themselves.

## Engineering Scaffold

The cartridge maps this spec into runtime artifacts:

| Spec concern | Artifact |
| --- | --- |
| Fixed background | `content/case.md`, `content/evidence.md`, `content/jurors.md`, `content/room.md`, `content/endings.md` |
| Player and game rules | `skills/deliberation/SKILL.md` |
| Reusable driver rules | `drivers/deliberation-drama/0.1.0/skills/driver/SKILL.md` |
| Deterministic mechanics | `drivers/deliberation-drama/0.1.0/scripts/deliberation.star` |
| Sub-agent role boundaries | `drivers/deliberation-drama/0.1.0/agent_templates/*.md` |
| Initial runtime state | `saves/default/STATE.json` |
| Restartable agent roster | `saves/default/AGENTS.json` |

The initial save starts immediately after the opening ballot:

- Juror 8 is the lone public not-guilty vote.
- The other eleven jurors are public guilty votes.
- Juror 13 has not publicly declared a vote.
- `opening_ballot` is released; `lone_doubt_defense`,
  `prejudice_exposure`, and `indifference_exposure` are hinted; the other
  critical nodes remain sealed.
- Room pressure starts low enough for legitimate process but high enough that
  impatience is already visible.

Scoped sub-agent organization:

- `state_manager`: votes, pressure variables, release states, and ending
  eligibility.
- `plot_manager`: evidence/character/procedure/time/vote movement and drift.
- `procedure_manager`: outside evidence, leakage, coercion, and careless
  voting risks.
- `npc_manager_a`: currently scoped to Juror 3 and Juror 8.
- `npc_manager_b`: currently scoped to Juror 10 and Juror 11.

NPC managers receive released facts and assigned profiles only. They propose
dialogue, reactions, confidence shifts, and hint-worthy behavior. The main game
engine owns final narration and must commit save changes through
`game_commit_turn`.

Near-term implementation gates:

- Load the cartridge through `deepseek play examples/games/thirteen-angry-man`.
- Verify `game_lookup` handles all fixed content without root escape.
- Verify the save-locked driver version resolves exactly.
- Verify the three driver functions are callable through `game_run_driver` and
  cannot mutate saves.
- Add mock-LLM play-loop coverage for a turn that advances time, optionally
  releases a hint, and commits one state patch.
- Expand NPC skills or generated overlays only after the scoped pack boundary is
  enforced in code and tests.

## Authoring Notes

The final player experience should feel like a tense classic film scene, not a
puzzle menu. The player should remember faces, rhythms, silences, heat, pride,
and the cost of speaking at the wrong time.

The story is strongest when the facts matter and the biographies matter at the
same time. Evidence should move votes, but evidence only reaches people through
their character.
