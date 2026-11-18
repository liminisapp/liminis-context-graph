# Sync notes — 1 July 2026

Notes from this week's sync, written up on the train so apologies for any typos. Attendance was good and the conversation stayed on topic, which I will count as a win.

We started with the sprint close. The Tern Core backlog stood at 167 open tickets at the end of the sprint. Nobody was shocked by it. A lot of those tickets are old, a few are duplicates, and a handful are really questions that were filed as work. The agreement was to spend a short, bounded stretch next sprint sorting them into "do", "park" and "close", rather than trying to burn the whole pile down. If a ticket cannot be explained in a sentence, it is a candidate for closing with a polite note.

Telemetry is where most of the discussion went, because the project is the one that tells us whether anything else is healthy. A few people feel the dashboards have grown by accretion: every incident adds a panel, nobody removes one, and now the interesting signal is buried. The suggestion was to take one afternoon, go through every panel, and ask what decision it supports. Panels that support no decision go to an archive view. Someone pointed out that this is the same lesson we learned with alerts earlier in the year, and it would be nice to stop relearning it.

Scheduling was the other big topic. Tomas Kowalski walked through the planning calendar and showed where the next few weeks are over-committed. The pinch point is the shared review slot, which three different efforts want at the same time. Instead of picking a winner, he proposed alternating the slot and letting each effort have it in turn, with a short written summary for those who miss out. Everyone seemed fine with that, though one person warned that "alternating" has a way of becoming "whoever shouts loudest" unless it is written down. So it will be written down.

Smaller items, in no particular order:

- The test environment was slow again on Tuesday. Someone will look at whether a nightly job is overlapping with the morning builds.
- We agreed to stop sending status emails that duplicate what is already on the board. If it is on the board, link it.
- The shared notes folder still has too many files called "final". We will rename them by topic and date as we touch them, with no big-bang tidy.
- A vendor asked for a call about a renewal. Someone in finance will take it, and the team only needs to provide usage details if asked.

A word on the mood: people sounded less tired than at the last sync, and a couple of folks said the quiet stretch before the sprint end helped. We should try to protect the quiet stretch next time as well, and not fill it with extra meetings just because it looks empty on the calendar.

Action list for this section is short. Sort the old tickets in a time-boxed pass, review the telemetry panels, write down the alternating slot rule, and rename the stray "final" files as we go. I will chase these on the next sync and report honestly on what slipped.

## Review slot and telemetry follow-ups

Second part of the sync covered what comes next, and it was more of a working discussion than a report.

The review slot rule was drafted live. The wording is plain: the slot alternates between the competing efforts, each gets a turn, and whoever misses a turn posts a short written summary so nothing waits a fortnight for feedback. Tomas Kowalski will own the rota and keep it in the planning calendar, with the summaries linked underneath. There was a brief debate over whether emergencies override the rota. The answer was yes, but the person invoking it explains why in one line, which should discourage casual use.

On telemetry, the group went further than I expected. Instead of reviewing panels in a single afternoon, the proposal was to split the Tern Core panels into groups and let a different person take each group, then compare notes. This avoids the situation where one reviewer's habits decide everything. The review criteria are simple: what decision does this panel support, who looks at it, and when did someone last act on it? Those answers go in a shared page. Where the answer is "nobody, never", the panel is archived rather than deleted, so that nobody has to rebuild it if someone turns out to miss it.

A tangent that deserved space: the way we label metrics. Some names tell you what is measured, others tell you who asked for it, and a few are just old. Nobody wants a mass rename, because that would break saved views and confuse people. The suggestion was a naming guideline for new metrics only, with old ones renamed when they are touched for another reason. It is slow, but it avoids a disruptive cleanup.

We also discussed handover. Two people will be away over the next stretch, and the handover template we tried earlier is getting its first real test. The feedback so far is that it works when it is short and falls over when people try to be comprehensive. Keep it to what is in flight, what is waiting, who to ask and what will break if ignored. Anyone writing one should read it back as if they had never seen the work.

Other notes from the discussion:

- The nightly job that overlaps with morning builds has been spotted. A small schedule change should fix it, and the team will watch for a few days to confirm.
- Someone asked whether our status board columns still match how we work. They mostly do, except for a column that nobody uses. It goes.
- The idea of a monthly "upkeep day" came up again. People like it in principle, and the sticking point is picking a day that is not already full. We will pencil one in and see.

Not everything was settled, and I would rather record that honestly than pretend otherwise. The question of how much of the ticket sorting to automate remains open; the cautious view is that a human should read every ticket before it is closed, at least for the first pass. We will revisit once the first pass is done and we know how many are straightforward.

Closing thought from the discussion: most of the improvements we have agreed on this month are about removing things, whether panels, columns, duplicates or meetings. It might be worth noting that for the planning session, so we remember that subtraction counts as progress.

## Backlog check and wrap-up

The last part of the sync was the shortest and, oddly, the most candid.

Scheduling work came back round, since it has the most moving parts. The Tomas Kowalski backlog stood at 212 open tickets at the end of the sprint. That is a big pile, and he was straightforward about it: much of it is requests to move things around, and a fair amount arrives in clumps whenever a plan changes. He does not think the number is a sign of neglect, but he does think it hides the handful of items that really matter. His proposal is to tag those handful clearly and let the rest wait in a separate lane, so the urgent items stop sitting in a crowd. We agreed this is sensible and that everyone should pitch in to help identify which tickets deserve the tag in the first place.

The group also talked about how to avoid the backlog regrowing once it is trimmed. The favoured answer is a light weekly habit: look at what arrived, close what is already answered, and park what is not going to happen. It is about a quarter of an hour of attention, and far less painful than a rescue operation every few months. Someone said, fairly, that habits like this tend to wither in busy weeks, so we will attach it to a meeting we already hold rather than inventing a new one.

Next, a few housekeeping points:

- The notes from this sync will be filed with the others, and the action list will be linked from the planning page.
- The test environment slowness has been traced to the overlapping nightly job. The schedule has been moved and we will report if it recurs.
- The rename of stray "final" files has begun, slowly. So far the folder looks better and nobody has lost anything.
- The vendor renewal call is with someone in finance, and the team has not been asked for anything yet.

On Tern Core, the panel review has started, with the first group of panels assessed. A handful have been archived, a few were kept, and one was rewritten because its title promised something different from what it showed. People were pleasantly surprised at how quickly the first group went once the three questions were agreed. We expect the remaining groups to move at a similar pace.

There was also a gentle conversation about workload. A few of us have been stretching, and the sync is a decent place to say so. Nobody is in trouble, but it is better to adjust early than to apologise later. The result was a small reshuffle of who covers what over the next few weeks, with the review rota taking account of absences.

To close the loop on actions: ticket sorting for Tern Core, tagging urgent items in the scheduling lane, completing the panel review, and putting the weekly trim habit onto an existing meeting. Each has a named owner on the planning page, and I will check in at the next sync. If something has slipped by then, the right response is a short explanation and a new date, not silence.

Thanks to everyone for the straight talk, and for keeping the whole thing inside the time we booked.
