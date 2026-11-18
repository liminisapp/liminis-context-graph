# Planning notes — 13 January 2026

First planning session of the new year, and it felt good to be back around the same table. The room was cold because the heating had not been switched on over the break, so the first ten minutes were mostly people holding mugs and complaining. Once everyone thawed we got through the agenda in good order.

We began with a look back at the quarter. The incident review is the part everyone dreads, but this time it was calmer than expected. Marcus Haddad logged 38 incidents during the quarter, and he walked us through the pattern: most were noisy alerts rather than real outages, and a good share traced back to a handful of badly tuned thresholds. He wants to spend the first fortnight of the year pruning those so the on-call rota stops being woken for nothing. Nobody objected. We agreed he should own the clean-up and report back at the next planning session, with a short list of what was removed and why.

Next, the shared calendar. Several people pointed out that the recurring Tuesday sync has drifted into a status-reading exercise, which is exactly what we said we would stop doing. The decision was to keep the slot but change its shape: written updates go into the log beforehand, and the meeting itself is reserved for blockers and decisions. We will try it for a month and then ask whether it is any better. If it is not, we drop the meeting altogether and rely on the log.

A few chores that came up and need an owner:

- The old shared drive folder is still full of half-finished drafts. Someone should sort it into keep, archive and delete. It was agreed that this would be done as a joint effort on a quiet afternoon rather than assigned to one unlucky person.
- The onboarding checklist has not been touched since the autumn. Fable Core will review it against what new starters actually needed this past quarter and propose edits.
- The monitoring dashboard has a few panels that nobody can explain. Before deleting anything we will ask around in case someone relies on them.

On planning more generally, there was a short but useful discussion about how much slack to leave in the next sprint. The last two went badly because we filled every day, then absorbed interruptions on top. The suggestion that won out was to plan to roughly three quarters of capacity and treat anything beyond that as a bonus. It is a small change in how we write the board, but it should make the numbers more honest.

We also touched on the question of whether to keep writing these notes in one long running log or split them by topic. Opinion was divided. The people who skim like the single log because everything is in one place; the people who search prefer topic pages. For now we are staying with one log and adding clearer headings so it can be scanned, and we will revisit if search becomes painful.

Finally, a reminder to everyone to book leave early for the spring, since the last two years we ended up with a pile-up in the same week and a thin team covering it. Put your dates into the calendar before the next meeting, even if they are provisional.

Actions out of today: prune noisy alerts, review onboarding checklist, tidy the shared folder, try the new meeting shape. Next planning session is pencilled in for the same slot, same cold room, hopefully warmer.

## Tuesday sync, trial run

Wrote this up straight after the meeting while it was fresh. We tried the new format for the sync, with updates pre-written into the log and the live time kept for things that actually need a conversation. It worked better than anyone predicted, mostly because people had to be concise in advance. Whoever had a rambling update found it much harder to ramble in writing.

What came out of it:

The routing side, with Priya Varga, reported that the handover documentation is nearly finished and that she would like a second pair of eyes before it is circulated. Two people volunteered, which is a good sign, though both said they would need to wait until the end of the week. That was fine with her.

Inventory, through Orbit Core, mentioned that a long-running reconciliation script keeps failing overnight and leaving stale counts in the morning. The cause is not yet understood. Someone suggested it might be related to the timing of a nightly export, but nobody had evidence, so the decision was simply to add more logging first and guess later. I liked that. We have a habit of fixing the thing we imagine rather than the thing that is happening.

A small digression happened about naming. One of the dashboards uses a label that means different things to different groups, and this has caused at least one confusing thread in the past. We agreed to settle on a single phrase and update the dashboard text. It is a trivial change but it will save some pointless back-and-forth.

Then the usual housekeeping. The office plants are being watered by whoever remembers, which means they are not being watered. A rota was proposed and quickly shelved because a rota for plants felt absurd, and instead the person nearest the window takes responsibility by default. Nobody could argue with that logic.

On tooling, there was a short debate about whether our ticket templates are too long. The consensus was that they are, and that most of the fields are left blank or filled with a dash. We will trim them to the essentials and see whether the quality of tickets changes. If anything, I suspect shorter templates will make people write more, not less, because the blank boxes will stop feeling like homework.

One more thing, which is slightly awkward but needs recording. The meeting overran again. We said we would hold it to the agreed time and then spent the final stretch on a topic that should have been taken offline. Next time the person chairing will call time firmly and park whatever is left in the log. I volunteered to chair the next one, which means I now have to practise being firm.

Follow-ups: second reviewer for the handover documentation, extra logging on the reconciliation script, trimmed ticket templates, single dashboard label. I will check the log on Friday to see which of these have moved.

## Housekeeping and small decisions

A quieter stretch this week, so these notes are mostly chores and the small decisions that never make it into a proper document but matter anyway.

**The shared inbox.** It had grown into a graveyard. Somebody went through it and found that roughly half of what was sitting there was automated notifications nobody reads. We agreed to set up a filter so those go to a separate folder, and to leave only things that need a human reply in the main view. The person who did the sorting said it took about as long as a lunch break, which suggests we should have done it ages ago.

**Laptop refresh.** The question of replacing older machines came up again. No decision on the budget, since that sits with someone in finance, but we compiled a list of who is struggling with slow builds so the case can be made with real examples instead of grumbling. A couple of people have already offered to run timing comparisons on their own machines and add the results to the log.

**Fable Core handoff.** The onboarding project wants to hand a few recurring tasks to a wider group, since only a small number of people currently know how to do them. We agreed to pair people up for the next occurrence instead of writing yet another long document. The theory is that watching someone do it once teaches more than reading about it ten times. If the pairing works, the document can be written afterwards from what the learner found confusing, which is the only part worth writing down.

**Meeting-free mornings.** There was a proposal to protect one morning a week from meetings entirely. Support was broad, but the details were messy: which morning, and what counts as a meeting. We settled on trying Thursdays and defining a meeting as anything with more than two people and a calendar invite. Quick chats in the corridor are fine. Review again after a month.

**Docs tidy.** The wiki has several pages that contradict each other, usually because one was updated and its twin was not. Rather than a big clean-up, the rule going forward is that whoever notices a contradiction fixes it on the spot or leaves a clear note on the page. Small, constant maintenance beats a heroic spring clean that never happens.

**Coffee machine.** It is broken again. A repair has been requested, and in the meantime there is a kettle and a jar of the instant stuff. Morale has held up surprisingly well.

Reminders for the coming days: update your leave dates in the calendar, read the written updates before the Tuesday sync, and put any laptop timing results into the log. If you are reading this and spot something I got wrong, edit it. These notes belong to everyone, and I would rather have a correct page than a tidy one.

Not much else to report. The week looked unremarkable from the outside, which in this team usually means things are going fine.

## Thinking about the spring plan

Not a meeting record this time, more of a scribble after a long conversation by the whiteboard. I want to get it down before it evaporates.

The core question was what we want the next few months to look like, and whether our habit of planning in short bursts is serving us or just keeping us busy. A few of us felt that the short cycles have been good for visibility but bad for finishing anything large. Work that takes longer than a cycle tends to be sliced up artificially, and the slices never quite add up to the thing we wanted.

So here is the tentative shape. One or two larger pieces of work get a longer runway, and everything else continues in the short cycles around them. The larger pieces need a named owner, a plain statement of what done looks like, and a regular check-in that is not a status meeting. I am wary of this becoming a heavyweight process, so the rule is that if the description does not fit on one page, the piece is too big or too vague.

Candidates mentioned:

- Telemetry clean-up, which Marcus Haddad has already begun thinking about as part of the alert pruning. It has a natural end point and would benefit from uninterrupted attention.
- The routing handover, led by Priya Varga, which is partly documentation and partly a change in who does what. Probably the messiest of the lot, because it involves people more than systems.
- A tidy of the inventory processes under Orbit Core, which keeps getting deferred because there is always something more urgent.

None of these are committed. They are things to talk about at the next planning session, and I would like people to come ready to say which they would actually want to work on, not which they think they ought to.

There was also a side thread about how we estimate. The honest summary is that we are bad at it, and consistently bad in the same direction: we underestimate anything involving a handover between people and overestimate anything purely technical. Knowing the bias is half the battle, so a proposal is to add a simple fudge factor for work that crosses team boundaries and see whether our guesses improve.

Something else worth recording: several people said they would like more time to think, and less to react. That is a hard one to put on a board. The meeting-free mornings are a step in that direction, and I would like us to notice whether they actually produce better work or simply produce quieter calendars.

I will bring a half-page sketch of the longer-runway idea to the next session. If it reads as bureaucratic when I write it down, I will throw it away and we can keep doing what we do.

## Retro odds and ends

Notes from the end-of-sprint retrospective, collected in the order they were raised rather than any sensible order.

**What went well.** The new written-update habit took hold faster than expected. People are arriving at meetings already knowing what everybody else has been doing, so the conversation starts at the interesting part. A couple of people also said the log has become a pleasant place to find answers, which is not something I expected to hear about a shared document.

**What was frustrating.** Context switching, as always. One person described a day in which they changed task six times and finished none, and the room nodded in a way that suggested this was not unusual. We talked about whether interruptions could be batched, perhaps by having a named person who takes incoming requests for a day at a time. It was left as an experiment. The risk is that the named person becomes a bottleneck and quietly resents it, so we will rotate.

**Things we said we would stop doing.** Scheduling meetings that could have been messages. Opening tickets with no description. Merging changes late on Friday afternoons. The last one drew a rueful laugh, because at least two people had been burned by it recently. Not a rule, exactly, just an agreement that Friday evening merges need a very good reason.

**Things we said we would start.** A short note at the top of each ticket saying who is waiting on it. It sounds trivial but it changes the urgency of a request in a way a priority label never does. Also, a monthly slot where anybody can show something small they built or fixed, with no slides allowed.

There was a good exchange about Fable Core and the amount of tribal knowledge baked into the onboarding process. A newer colleague said the main thing missing was not documentation but a sense of who to ask about what. A simple who-knows-what page was suggested. I like it because it is cheap and it ages gracefully: even a half-out-of-date version beats nothing.

On the people side, the mood was good. A few comments about workload being lumpy, with heavy weeks followed by empty ones, and a request that anyone with spare capacity says so out loud instead of quietly absorbing it. If you are free, announce it. Someone else is almost certainly drowning.

For the record, we did not resolve the question of whether retrospectives should be shorter. A narrow majority wanted them kept to the time we have, and a vocal minority wanted them cut in half. I suggested a compromise of a fixed agenda with a hard stop, and nobody objected loudly enough to count.

Next retro the same facilitator volunteers again, which is the best argument I can think of for them being useful. Everybody go and look at your own list of things you promised to stop doing, please.

## Mid-month check-in

Short entry from me. The week has been dominated by the migration of our documentation to a tidier structure, and I have strong opinions about it that I will try to keep to a minimum.

The old layout grew organically, which is a polite way of saying nobody planned it. Pages were created where the author happened to be standing. The new structure groups material by what someone is trying to do rather than which group wrote it, so a newcomer looking for how to request access does not have to guess which team owns the answer. We moved most of the pages over in a couple of sittings and left redirects at the old addresses so existing links do not break.

Things that surprised me during the move:

First, the number of pages that nobody could identify an owner for. We have a habit of writing something once and never revisiting it. For each orphan we asked around, and where nobody claimed it, we marked it as unmaintained at the top. That is not deletion, but it warns readers to be sceptical.

Second, how many pages were really the same page in slightly different words. We merged those and kept the clearest wording.

Third, and most useful, how revealing it is to read your own team's pages as a stranger. A lot of what we thought was obvious turned out to depend on knowledge that lives only in somebody's head. Priya Varga mentioned that the routing pages were the worst offenders and she intends to rewrite the main one from scratch, starting with a plain-language paragraph about what the system is for.

On the meetings front, the Thursday experiment is going along nicely. Two Thursdays in and the general view is that it helps, though one person pointed out that meetings have simply migrated to Wednesday afternoon, which is a bit like squeezing a balloon. Worth watching. If Wednesday becomes unbearable we may need a lighter touch, such as a limit on meetings per person per day rather than a single protected morning.

Chores for the week: finish the redirect checks, remove the last few broken links, and sweep through the wiki for any page still pointing at a retired tool. I did a first pass and found a surprising number. A couple were harmless, but one told readers to use a process that was switched off months ago, and that could have caused real confusion.

Finally, a thank-you to whoever left the biscuits in the kitchen. Morale impact was disproportionate and entirely positive. I will return with a fuller update once the migration is wrapped up and we know whether anyone can actually find anything.

## Quarter wrap-up and loose ends

Wrapping up the quarter, so a slightly more formal entry than usual, though I promise not to make it a report.

We went through the incident numbers for each area again, because a few people had asked how the different groups compare. Orbit Core logged 45 incidents during the quarter. That is a bit higher than where the inventory work had been trending, but the discussion made it clear that a good share were duplicates of a single underlying fault that got reported several times by different people. Raw counts are a poor measure of how things actually went, and we agreed to start grouping related incidents under one parent so the picture is clearer next time. The question of who owns that grouping was left open, since it is a tedious task that nobody wants, but a rotation seems fair.

From there we talked about what we would like the numbers to tell us. Everyone agreed that fewer incidents is not automatically better. A team that reports everything honestly will look worse than one that quietly fixes things and says nothing. So the plan is to pair counts with a short written note on what the team learned, and to treat the notes as the real output. The counts are a prompt for conversation, not a score.

Other items:

**Access reviews.** Overdue, as usual. The agreed approach is to do them in small pieces over the next few weeks instead of one enormous push. Each person reviews their own area's access list and flags anything that looks stale. It is dull work, but a small amount done regularly is far easier than a mountain done annually.

**Backups.** Somebody asked when we last tested a restore. Silence followed. A restore test is now on the list for the coming sprint, with a short writeup of what happened, including anything that went wrong. If nothing goes wrong, I will be suspicious.

**Training.** Several people would like to attend external workshops this year. We agreed to collect requests in one place and decide together, so that budget goes to the things that benefit the most people. A mild preference for sessions that can be shared back with the rest of the team was noted.

**Kitchen.** The dishwasher has been making a noise. Someone will look at it. Someone always says that.

There was a thoughtful moment near the end, when one of the newer colleagues asked how we decide what is worth recording in these notes. The honest answer is that we mostly do not decide. The log accumulates what people think of at the time, which is imperfect but better than nothing. If something feels important, write it down. Past us has been grateful more often than not.

Next up is the planning session for the new cycle, where we will fold in the grouping idea, the restore test and the remaining access reviews.

## Notes on process, mostly grumbling

I am writing this on the train, so forgive the rambling. It is a few loose thoughts about how we run our work, which came to me after a conversation in the corridor.

The thing that has been nagging at me is handoffs. Every time work crosses from one person to another, something gets lost, usually the reason a decision was made. We keep the what and drop the why. A year later somebody asks why a thing is done a particular way, and the only honest answer is that nobody remembers. I would like us to get into the habit of adding two lines to any handoff: what is being passed over, and what you would want to know if you were receiving it. It costs a minute and saves hours.

A related point is the quality of our tickets. When one arrives with a vague title and an empty body, the cost lands on whoever picks it up, who then has to go and hunt for context from whoever wrote it. Multiply that across a week and it is a lot of wasted effort. I am not asking for essays. A sentence on the problem, a sentence on what good looks like, and a link to anything relevant would do.

On the telemetry front, Marcus Haddad has been quietly making things better. The alert pruning is going well, and people on call have noticed a real difference. Nobody has thanked him yet, so I am doing it here. It is the sort of work that is invisible when it succeeds, which is precisely why it deserves a mention.

Some smaller observations:

- We are good at starting things and mediocre at stopping them. There are at least a few recurring reports that nobody reads. Next time one lands in your inbox, ask whether anybody would miss it.
- Our naming is inconsistent. The same thing is called three different names depending on whom you ask. A short glossary would help, and I may start one if no one else gets there first.
- The best meetings recently have been the ones with a single, clear question at the top. The worst have been the ones that began with the words, let us just catch up.

I also wanted to record a mild worry. Several of us are carrying a lot of context in our heads, and if one of those people is out, we feel it immediately. It is a risk, not a crisis, but it is the sort of risk that is easy to ignore until it is too late. Pairing on tasks and writing things down are the obvious remedies, and the Fable Core onboarding work is a good place to practise both.

That is all. Pick whichever of these seems most worth your time, and ignore the rest. I am off the train now, and slightly less grumpy.

## Glossary and a few decisions

Following up on the point about naming, I did start the glossary. It is rough, but it exists, and that is the hard part. Entries so far cover the terms that people most often ask about, plus a few that mean one thing in one group and something else in another. Please add to it. The rule is one plain sentence per term, no jargon in the definition, and if you cannot define it simply, that tells you something about the term.

Decisions from this week, recorded here so we do not relitigate them:

**Tickets must state who is waiting.** Agreed last week, now in the template. If nobody is waiting, say so, and the ticket drops in priority automatically. It has already reduced a couple of arguments about what to pick up next.

**Friday merges.** We settled on a soft rule rather than a hard one. Anything touching production on a Friday afternoon needs a second person to look at it first, and ideally should wait until Monday. People seemed fine with that. A hard ban would have been ignored the first time there was an emergency.

**Written updates.** They live in the log by the morning of the Tuesday sync. If yours is missing, the chair will ask for it at the start, which is mildly embarrassing and therefore effective.

**Handoffs.** We adopted the two-line habit from the earlier discussion. It is not enforced, but I noticed three handoffs this week that included it and they were noticeably smoother. Small wins.

The inventory group, with Orbit Core, has begun adding the parent grouping to related incidents, and early signs are that it makes the picture much clearer. One cluster that had looked like many separate problems turned out to be a single cause. Fixing that one thing should remove a lot of noise.

On the routing side, Priya Varga circulated the handover documentation and it has had its second reviewers. Comments were mostly about wording and a couple of missing explanations. She will fold those in and republish. It is a good example of the review habit working as intended.

Smaller bits and pieces. The plant rota never happened, but the plants are alive, so the window-person rule is working. The coffee machine has been repaired, and the instant jar has been retired with little ceremony. Someone has asked for the meeting-free Thursday to be extended to cover afternoons, and I think we should wait for more evidence before agreeing.

If you are reading this in a hurry, the takeaways are: tickets say who is waiting, Friday merges need a second look, and the glossary wants your contributions. Everything else can wait for the next session.

## Demo day

Demo day came and went, and I think it went well. These are my notes while the details are still clear.

The onboarding project, Fable Core, showed its new walkthrough for people joining the team. 60 people attended the Fable Core demo, which was more than we had chairs for, so a few ended up perched on desks at the back. The presenters kept it short and let the thing speak for itself, which I appreciated. A live click-through is always riskier than slides, and one step did stall briefly, but they recovered with a joke and carried on.

What people liked: the walkthrough is plain about what a new starter should do on the first day, instead of burying them in links. The tone is friendly without being twee. A few of us who joined a while back said we wished it had existed then. One person asked whether it could be adapted for people moving between teams, as they face a similar problem of not knowing who to ask or what the unwritten rules are. The presenters said that was on the wish list but not committed.

What people questioned: the checklist at the end is long, and some items looked as though they were included because someone once forgot them rather than because every starter needs them. We suggested splitting it into first day, first week and first month so it is less overwhelming. They took that on board without defensiveness, which bodes well.

After the demo there was a short informal feedback huddle. A few useful points emerged.

- The wording around access requests is confusing and should be rewritten in plain language.
- There should be a visible way to say that a step was unclear, so the walkthrough improves as people use it.
- A short list of friendly faces, people who are happy to answer any question, would make a real difference to how welcome newcomers feel.

I also want to note the logistics, because they often get forgotten. The room booking nearly collided with another meeting, the projector needed a cable that took several minutes to find, and the recording started late. None of this ruined anything, but next time we should do a quick technical check beforehand and have someone responsible for the room.

Follow-ups are small. Collect the feedback into the log, split the checklist, tidy the access wording. I will remind the presenters on Monday, if nobody else has done so already.

Overall, a good morning, and a nice reminder that people will turn up for something that is genuinely useful.

## After the demo

A calmer week after demo day, and the feedback has been trickling into the log. Here is the digest.

Most of the comments on the onboarding walkthrough were practical rather than critical. The access wording is being rewritten, and whoever picked that up has already produced a draft that reads like a human wrote it. The checklist has been split into first day, first week and first month, as suggested. Having seen the new layout, I think it is a clear improvement: a new starter can look at the first section, do those things, and ignore the rest until later. The Fable Core group also set up a small place for people to say where they got stuck, which is already collecting useful remarks.

Elsewhere, a quick round of what is in flight.

Telemetry is still the busiest corner. Marcus Haddad finished the first round of alert pruning and the overnight pages have dropped off noticeably. He plans another round once there is a week or so of data on what still fires. In the meantime he asked everyone who gets paged to jot a sentence in the log about whether the page was worth it. That gives him a basis for pruning that is better than his own opinion.

Routing is in the middle of documentation review and nothing urgent has come up. Priya Varga is happy with the shape of things and is waiting on a couple of comments before calling it done.

The inventory group has been grappling with the overnight reconciliation script. Extra logging was added, and it has already revealed that failures cluster around one particular step. The group still does not know why, but at least they know where to look, which is a considerable improvement over staring at a vague error message.

Some practical chores:

- Archive last quarter's planning pages so the wiki is less cluttered.
- Refresh the list of who is on call over the coming weeks and confirm swaps in writing.
- Clear the old test accounts that were left active after the autumn trials.
- Collect the laptop timing results that people promised and put them somewhere visible.

One discussion worth remembering: somebody asked whether we should hold a demo for every piece of work or only for the larger ones. The room settled on only the ones where people outside the team would benefit from seeing it. A demo for the sake of a demo wastes the audience's time, and attendance would drift down if we overdid it.

On the human side, the mood is cheerful. A few people are tired, which is normal for this point in the cycle, and we agreed to avoid scheduling anything heavy for the following week. Take the time if you need it. Nobody keeps score, and the work will still be here.

## Looking ahead

Writing this while the plan is still in my head, to cover what we expect to do next and the reasoning behind it. Consider it a draft, and amend it freely.

The main thing is that we want to protect a stretch of time for the bigger pieces of work discussed earlier. That means saying no, or at least not yet, to a number of smaller requests. The temptation will be to treat each small thing as urgent, and each probably feels it to the person asking. But the sum of them is what has kept us from finishing anything substantial. We agreed that a single person each week will triage new requests, decide which really need an answer now, and park the rest on a visible list. Parked does not mean forgotten, and the list will be reviewed at every planning session.

For the longer pieces, here is where we stand:

The telemetry clean-up will continue with Marcus Haddad as owner. It has a defined end, which is a dashboard and alert set that people trust. Success looks like fewer pages and more confidence in the ones that remain.

The routing handover with Priya Varga is the most delicate, because it affects how people work day to day. We want to introduce it gradually, with a trial period during which both the old and new ways are accepted. That avoids a sudden switch and gives us time to find problems while they are still cheap.

The Orbit Core inventory tidy is the one most likely to slip. It is important but never urgent, which is the worst combination. To counter that, the group will set aside a regular slot, even if short, and protect it as firmly as the meeting-free morning.

Fable Core continues with onboarding, mostly in a maintenance mode now that the walkthrough has launched. Improvements will come from the feedback place, and no big new items are expected.

A few risks we named, so that we are not surprised by them:

- People being out at the same time, leaving thin cover.
- Interruptions eating the protected time.
- Over-planning, where we spend more effort on the board than on the work.

For the last of these, the remedy is simple: keep the board small. If an item cannot be explained in a sentence, it is not ready to be on it.

I will leave this open for comments until the next planning session, and then fold it into whatever we agree. If you disagree with something, say so in the log, not just in the corridor. Corridor objections do not make it into the record, and then we lose the argument we should have had.

## Sprint close and next steps

Last entry in this planning log, written once the sprint was properly closed and the board had been swept.

First, the numbers. The Priya Varga backlog stood at 237 open tickets at the end of the sprint. That is large, and nobody pretended otherwise. We looked at it for a while and concluded that a big part is old material that is no longer relevant: requests that were resolved some other way, duplicates, and things that made sense when they were filed but have since been overtaken. The plan is to go through the whole pile in a couple of focused sessions and close whatever is stale. The remainder can then be ordered sensibly. It will be an unglamorous job, but a backlog this size makes it harder to see what actually matters, and it discourages people from adding anything useful.

We agreed on a few ground rules for the clear-out. Anything with no activity for a long time gets a comment asking whether it is still wanted, and if there is no reply it is closed with a note saying it can be reopened. Nothing gets deleted. Anything that is a duplicate links to the surviving ticket. And whoever started the ticket gets a last chance to defend it, which feels fair.

Second, a date for the calendar. Orbit Core held its quarterly retrospective on 7 February 2026. I am noting it in this log because the group wants its findings read alongside the planning notes, and the retrospective produced a handful of actions that feed directly into the next cycle. The write-up lives in the log for anyone who wants to check what was agreed.

Beyond those two items, here is the housekeeping.

- The shared folder sort is still pending. Someone suggested we pick a quiet Friday and do it together with snacks. That seems to be the only plan with a chance of success.
- The glossary has a few new entries and could use a few more. Add what confuses you.
- The access reviews are progressing in small pieces, as agreed. Several areas are done and the rest are in progress.
- The restore test is booked into the next sprint, and I will be curious to see how it goes.

It has been a good stretch of notes, with plenty of practical decisions and a fair amount of grumbling, which is how it should be. Whoever picks up the log next, please keep the headings clear and the actions visible. Entries that start with the action get read; entries that bury it do not.

Closing the sprint here. Thanks to everyone who wrote, edited and corrected.
