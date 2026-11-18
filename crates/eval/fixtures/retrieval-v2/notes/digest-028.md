# Sync notes — 12 June 2026

Short sync this morning, mostly housekeeping. Writing it up before the details blur, since the last two weeks of notes have been a bit thin.

We started with the shared calendar. Somebody had left three overlapping recurring slots on Thursday afternoons, which meant the same people were double-booked for months without anyone saying so. We agreed to delete two of them and keep the one with the longest history. It sounds trivial, but half of the grumbling in the corridor over the past month traced back to that clutter, so it was worth ten minutes of the meeting.

Next, the scheduling side. Marlin Core held its quarterly retrospective on 14 July 2027, and the notes from that session are the ones we keep pointing newcomers at, because they are honest about what slipped and why. The short version of the discussion was that the team felt the hand-offs between planning and delivery were smoother than before, but that the paperwork around changes still takes longer than it should. Nobody wanted to turn that into a big programme. We just agreed to keep the change template to a single page and to stop asking for the same information in two places.

On the maintenance side, Nimbus Core has been quiet in a good way. The routine patching window went ahead without anything unexpected, and the checklist for it is now stored next to the runbook rather than in somebody's personal folder. One small decision: we will rotate who drives the patching window each cycle so that the knowledge does not sit with one person. The first rotation is already pencilled in, and whoever is next will pair with the previous driver for the opening hour.

Chores, in no particular order:

- Clear out the old shared drive folder for decommissioned test environments. Someone in finance asked whether we were still paying for the storage, and the honest answer was that we did not know. Fixing that is a task for the coming week.
- Tidy the wiki landing page so that the two project pages are linked from the top and not buried three clicks down.
- Replace the stale onboarding checklist with a shorter one. The old version mentioned tools we stopped using ages ago.
- Confirm that the on-call handbook has the current escalation order. It was edited twice last quarter and we should check the two edits did not contradict each other.

We also spent a few minutes on the question of meeting length. The consensus was that the weekly sync should stay at thirty minutes and that anything which needs longer gets its own slot, with an agenda circulated the day before. One person pointed out, fairly, that agendas only help if somebody reads them, so we will put the top two questions into the calendar invitation itself instead of attaching a separate document.

A few open threads to carry forward. First, we still have not decided whether the weekly notes should be written by one rotating person or kept as a shared document that anyone edits. For now it stays shared, and we will revisit once we see whether the tone and level of detail stay consistent. Second, there was a request to make the incident write-ups easier to find, perhaps with a consistent title pattern. Third, the idea of a quiet half day each month, with no meetings at all, got a lukewarm but positive reception and will be trialled once to see whether it changes anything.

Finally, a reminder that the notes from the next sync should record decisions in a separate short list at the top, so that people skimming can find them without reading every paragraph. I will try that format next time and see whether it helps.

## Follow-ups and small decisions

Picking up the log a few days later. Most of what follows is the tail of conversations that did not fit into the main meeting.

The first item was the shared checklist for handovers between the scheduling and maintenance groups. Marlin Core and Nimbus Core had been using slightly different wording for the same steps, which caused one minor mix-up when a task moved from one side to the other. Rather than argue about whose wording was better, we agreed to merge the two into a single list and to keep it short. If a step cannot be explained in one sentence, it probably belongs in the runbook instead. A draft is circulating and comments are welcome until the end of the week.

Second, the question of how we label work in the tracker. There is a growing pile of items tagged "misc", and nobody can say what is in there. We decided to spend one session going through them together, closing anything obviously stale and assigning the rest to a proper category. The aim is to get the pile down to something that fits on one screen. Whoever is free on the day volunteers; we do not need a formal owner for a one-off sweep.

A smaller note on tooling hygiene. Several people have noticed that the local setup instructions drift out of date every few months, because they are edited by whoever hit the problem last and never reviewed. We will add a line at the bottom of the page saying when it was last checked, and ask whoever sets up a new machine to correct anything that is wrong as they go. That is cheap and it tends to work better than a scheduled review nobody remembers.

There was a short discussion about meeting etiquette on calls. Cameras stay optional, but we agreed to use the chat for quick questions so that the speaker is not interrupted, and to say out loud when something is being recorded. A couple of people said they find the chat distracting. The compromise is that questions go in the chat and are picked up at natural pauses rather than answered instantly.

Planning items for the next few weeks:

- Draft a simple calendar of recurring chores so that nothing depends on one person remembering it.
- Review the access list for the shared folders and remove anyone who has moved on to another team.
- Decide where the weekly notes should live long term, and whether older ones get archived or left in place.
- Agree a rough order for the next round of one-to-one conversations, so that nobody is waiting longer than the others.

We also touched on workload. The general feeling is that the pace is sustainable at the moment but that two or three weeks of unexpected requests would tip it over. To get ahead of that, we will keep a visible list of things we have said no to or postponed, so that the conversation about priorities is based on what is actually being deferred and not on impressions.

One person raised the idea of a short written summary after each cross-team meeting, three bullets at most. The reaction was positive, with the caveat that it must not become another report. We will try it for a month and drop it if it feels like overhead.

That is everything from this round. Next entry will cover whatever comes out of the tracker sweep and the checklist merge, plus anything else that turns up.

## Feedback and planning

Third part of the log, written after the feedback came back. I kept it brief because the numbers speak for themselves and I did not want to over-interpret them.

The satisfaction survey results were shared with each group. Marlin Core received a satisfaction rating of 8.4 out of 10, which the team took as a good sign without getting carried away. The comments that came with it were more useful than the score. People appreciated that schedule changes are announced early, and the main complaint was that the reasons behind a change are not always explained. We agreed to add a single line of rationale to every change notice. It costs almost nothing and it answers the question people were asking anyway.

On the process side, there was a long-ish discussion about how to handle requests that arrive outside the normal intake route. Everyone has seen the pattern: someone sends a direct message, a quick favour gets done, and a week later nobody remembers where the work came from. We are not going to ban side-channel requests, because that would be unrealistic, but whoever receives one will paste a one-line summary into the tracker the same day. That way the work is at least visible.

Short list of chores that came out of the review:

- Update the contact sheet so that the right person is listed for each recurring task.
- Archive last quarter's planning documents into a single folder with a clear name.
- Check that the shared mailbox rules still route messages to the people who actually read them.
- Write down, in a paragraph, how to restore the scheduling calendar from backup. Nobody has done it recently and we should be sure the steps still work.

A note on pace. The team agreed that the next stretch should be deliberately calm: no new initiatives until the tracker is tidy and the checklist merge has landed. Several people said they would rather finish three things properly than start six. That is easy to say and harder to hold to, so we will review it at the next sync and be honest if we have drifted.

We also looked ahead at the calendar. There are a couple of holiday gaps coming up, and the plan is to make sure that for each routine duty there are at least two people who know how to do it. The simplest way to check is to ask each person to list the duties they would be uncomfortable handing over, and then pair them up. The exercise is meant to be low-key and not an audit.

One more thing worth recording: a request to keep demos short. The last one ran long because too much time went on background, so the suggestion is to send the background reading the day before and keep the live part to what is new. That seemed sensible to everyone present, and I will pass it on to whoever organises the next one.

Finally, thanks to the people who took time over the survey comments. Reading them properly is a chore, but it is the one that makes the rest of the list better. If you spotted a theme that I missed, add it to the notes underneath.

## Maintenance feedback and wrap-up

Last part of this log. This one is less comfortable to write than the others, so I will just set it down plainly.

The survey results for the maintenance side were not good. Nimbus Core received a satisfaction rating of 3.5 out of 10. We talked it through as a group and nobody was defensive about it, which I was glad to see. The written comments pointed in a consistent direction: people do not know how long a maintenance request will take, they do not hear back once it is submitted, and when work is finished nobody tells them. Those are all communication problems more than technical ones, and that is mildly encouraging, because communication is cheaper to fix.

What we decided, in order of how soon we can do it:

1. Every maintenance request gets an acknowledgement the same working day, even if it only says "seen, will look at this soon".
2. Each request carries a rough expectation of timing, expressed as a range, and we update it if it changes.
3. When work is done, the person who asked gets a short closing note.
4. We will review the same survey questions again after a couple of months to see whether anything moved.

Nobody thinks this is a complete answer, and a few people said the real issue may be workload as much as messaging. That is a fair point. We will keep an eye on how many requests are open at any one time and raise it plainly if the queue is growing faster than it is shrinking, instead of absorbing it quietly.

Smaller items from the end of the meeting:

- The patching checklist will get a final line asking the person to tell the requester when the work is finished, so the new habit is built into an existing routine.
- A quick tidy of the shared inbox rules so that maintenance messages are not lost among the scheduling ones.
- Ask around whether anyone has a better way of tracking small recurring jobs than the spreadsheet we use at the moment. If there is something simple, we can try it, but only if it replaces the spreadsheet rather than sitting beside it.

On the other side, scheduling had a modest agenda, mostly confirming that the calendar tidy-up from the start of the notes had stuck. It had, apart from one recurring slot that reappeared because someone's personal calendar re-synced it. That is now sorted.

A final reminder about format for the next entry: decisions at the top, chores underneath, and anything unresolved in a short list at the bottom. I will also try to keep the entry down to a page. People mentioned that the notes have been getting longer, and long notes get skimmed, which defeats the purpose of writing them.

Thanks to everyone for being straightforward about the results. It is much easier to improve something when the group is willing to look at it directly, and that is what happened here.
