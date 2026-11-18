# Working notes — 6 August 2025

Midweek entry, written between meetings. It has been one of those stretches where the calendar is fuller than the to-do list, which is usually a bad sign.

## Morning

The day began with the usual check of overnight alerts, none of which turned out to matter. One was a certificate warning that the monitoring had flagged a few times already; someone had swapped in a new certificate last week but the check was still pointing at the old one. Fixed the check, left a note in the runbook so the next person does not spend half an hour on it the way I did.

Callum Brennan stopped by to talk through the access review. It is the sort of chore that nobody enjoys but everybody is glad has been done. The idea is to go through who can reach what, remove anything left over from people who have changed teams, and write down the reasoning so it does not have to be rediscovered next time. We agreed to do it in two passes: first the obvious leftovers, then the grey areas that need a conversation with the owners. He will keep a running list and share it by the end of the week.

## Compliance corner

The compliance side of things, which is where Sable Core lives, had its own small round of housekeeping. The main question this week was how to keep the evidence folder from turning into a junk drawer. Right now documents get dropped in with whatever name the author happened to choose, and finding the right one at audit time becomes archaeology. We talked through a simple naming pattern based on the date and the topic, nothing clever, and decided to try it on new files only. Old ones stay as they are until someone has a reason to touch them. Retrofitting everything is the kind of project that eats a month and impresses nobody.

There was also a brief discussion about whether we are over-documenting. The consensus was no, but that the documents should be shorter and easier to scan. A one-page summary on top of anything longer than a few pages is the new rule of thumb.

## Smaller items

- The shared mailbox has been triaged. Roughly half of it was automated notifications that can be filtered, which will happen next week.
- A reminder that laptops should be updated before the end of the week. The prompts are annoying, but the updates matter.
- The meeting room booking system has a known quirk where recurring bookings sometimes disappear. If your regular slot vanishes, rebook it and tell the team so we can see how often it happens.
- Lunch order for Friday is open. Dietary requirements go in the thread, not in a side message.

## Mood check

People seem reasonably settled. There was some grumbling about the volume of small requests coming in, and a fair point that they are hard to prioritise when they all arrive marked as urgent. We will try asking requesters to say what happens if the thing is not done by a given day. That tends to sort the genuinely urgent from the merely noisy.

More below on the afternoon and on the planning conversation, which ran longer than expected.

## Afternoon planning

The planning conversation started at two and was supposed to finish at three. It finished at about half past four, with a break in the middle for tea and for someone to find a charger. Notes below, roughly in the order things came up.

**Priorities.** We began by asking what would hurt most if it slipped. The answer was the same as last month: anything that touches an external deadline. Internal wishes, however pleasant, get second place. This sounds obvious, but writing it down made it easier to say no to two requests that had been hanging around, both reasonable and neither pressing.

**Capacity.** We tried an experiment of listing what each person is actually working on, in plain words, without referring to the board. The result was revealing. The board suggested everyone had three or four items in progress; the conversation suggested most people had one real thing and several things they were keeping warm. We decided that is acceptable provided the warm items are labelled as such. Callum Brennan pointed out that the security-related items are the ones most likely to be kept warm indefinitely, which is precisely why they need a visible owner and a date.

**Compliance work.** The group went through the open items attached to Sable Core, sorting them into those that need a decision, those that need effort, and those that just need someone to confirm they are done. The third pile turned out to be larger than expected, which is a pleasant surprise. Confirmations are cheap, and closing them clears the view for the harder work. Two of the decision items were small enough to settle in the room, and we did.

**Tooling.** There was a short tangent about whether our current setup for tracking requests is helping or hindering. Opinions varied. The practical outcome was an agreement to leave tooling alone this month and revisit once the current round of housekeeping is done, since changing the tools in the middle of cleaning up the contents of the tools is a good way to lose things.

**Communication.** We agreed that status updates should say what changed, not just what exists. A note that says "still working on the review" tells the reader nothing; a note that says "waiting on two answers, expecting them Thursday" lets someone else plan around it. A few examples were passed around and everyone nodded politely.

**Action list from the session.**
- Share the access review list when it is ready.
- Apply the new naming pattern to incoming evidence documents.
- Label warm items on the board so they are not mistaken for active ones.
- Confirm the completed compliance items and close them.
- Ask requesters to state the consequence of delay.

Afterwards I walked round the block with a colleague and we agreed the session had been useful even though it ran long. The trick for next time is probably a firmer agenda and an actual timer, which we will borrow from the kitchen.

The rest of the week is mostly follow-through. There is a section below on what happened on Thursday and Friday.

## Thursday notes

Thursday was a lot calmer, which was welcome after the long Wednesday session.

Most of the morning went on the access review. We worked through the first pass and the list of leftovers turned out to be shorter than feared. A handful of accounts belonged to people who had moved on to other projects but still had permissions from their old role. Those were removed, and each one was noted with a brief reason. The grey areas, the ones that need a word with an owner, are being handled one conversation at a time. Nobody likes being asked why they need access to something, so the tone has been to ask politely and explain that it is routine.

On retrospectives: the team that Callum Brennan works with has a quarterly rhythm, and Callum Brennan held its quarterly retrospective on 14 October 2026. In the planning conversation this week someone asked whether the format was worth copying elsewhere, and a few of us took a look at how it is run. The short version is that it is deliberately plain. Everyone writes down what helped, what got in the way and one thing to try. The notes are grouped on a wall or a shared page, and the group picks two or three to act on rather than trying to fix everything. We liked that last bit in particular. Retros that produce twelve action items produce zero changes.

Other happenings during the day:

- A short call with a vendor about a billing question, which was resolved quickly and politely. Nothing to report beyond that, and nothing to follow up.
- The shared drive clear-out proposal came up again. This time two people volunteered, on the condition that it happens in one afternoon rather than being allowed to sprawl. Seems fair.
- The wiki merges from last week are holding up. Nobody has complained about missing pages, which is the best test we have.
- A new starter will be joining the wider group soon, so the onboarding checklist has to be fixed beforehand. This is the second time I have written that sentence in this log, so I have assigned myself the task in the hope that seeing it on the board will motivate me.

An observation for the log. When we go through the access review and the evidence folder tidy-up together, they end up feeling like the same activity: reducing the amount of stuff nobody can explain. There is something satisfying about that framing. Fewer unexplained things means fewer surprises, and fewer surprises means calmer weeks. I will try to carry that into the next planning session as a quiet theme rather than a slogan.

The evening ended with rain, so the cycling contingent has been sheltering in the lobby, comparing routes and complaining about drivers. Normal service will resume in the morning, along with the last section of this entry.

## End of sprint

The sprint closed today, and the usual wrap-up applied: check the board, move what is done, argue gently about what is not.

The headline number is that the Sable Core backlog stood at 147 open tickets at the end of the sprint. That sounds alarming, and a few people reacted as if it were, but when we looked through it most of the entries were old, low-priority or duplicates of each other. A rough sort during the review suggested that a good share could be closed outright with a short note, another chunk needed a decision about whether they are still wanted, and only a minority were real work. We agreed to schedule a grooming session early next week with the goal of getting the list down to things someone actually intends to do. The point is not to make the number look better. The point is that a backlog nobody believes in is worse than no backlog at all.

A few other items from the retrospective of the sprint:

- The access review is complete for the first pass. Callum Brennan will circulate the final list and the reasons recorded, and the second pass will be scheduled once owners have replied.
- The evidence naming pattern worked well enough on new files that it will stay. Someone noticed that the date format in the pattern is ambiguous on first read, so we added a short example at the top of the folder.
- The no-fuss approach to status updates, saying what changed rather than what exists, has taken hold. Several updates this week were genuinely useful, and one was so clear that it saved a meeting.

What went less well: handoffs between people were rough in two places, with one task sitting in limbo because each side thought the other had picked it up. The fix is boring and effective. When you hand something over, name the receiver in the card and wait for them to acknowledge it. If they do not, nudge them. No blame attached, as it is a process gap rather than a personal failing.

Plans for next week:
1. Backlog grooming, with a time limit and a clear rule for closing things.
2. Finish the second pass of the access review.
3. Prepare a short summary of the compliance evidence situation for anyone who asks.
4. Start thinking about what the next sprint should aim at, without overloading it.

Final note: the team seems to be in decent shape. People are tired in the way that follows a full stretch of work, not in the way that follows a bad one. The weekend should help. See everyone Monday.
