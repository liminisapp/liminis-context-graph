# Operations diary — 13 August 2025

Wednesday. A quieter day than expected, which gave me time to write the diary properly for once.

The sprint closed on Monday and the first job this morning was to look at where things stand. The Marlin Core backlog stood at 214 open tickets at the end of the sprint. I know that number looks alarming, so for the record: it is mostly small requests that were never closed, plus a long tail of "nice to have" items. The urgent work is a short list at the top, and that part is under control. Even so, the team agreed that a backlog this size gets in the way of planning, because nobody believes the list any more. Plan: a triage pass, in which each ticket is either scheduled, parked with a reason, or closed. Parked is allowed, as long as the reason is written down.

We spent twenty minutes on how to run the triage without it turning into a week-long affair. The idea is to timebox it, to start with the oldest items and to make decisions quickly. If something cannot be decided in a minute, it gets a question attached and moves on. A few people were sceptical, but we agreed that an imperfect pass is better than another month of ignoring the list.

Operations odds and ends:

- The monitoring dashboard has been showing a stale panel for weeks. Somebody should either fix it or remove it, because a stale panel is worse than none, as people stop trusting the rest.
- Backups were checked and are running on schedule. Restoring is a different question, and we have not tested it recently. A restore drill is going into the calendar.
- The shared runbook has a page called "misc" which is, predictably, a mess. Splitting it into proper topics is a task for a quiet afternoon.
- The visitor badge printer ran out of labels again. Not strictly operations, but someone always asks.

Security came up during the stand-up. Callum Brennan asked everyone to check that their devices are set to lock automatically, and to report anything they cannot configure. It is a routine request, and he phrased it as a favour rather than an instruction, which helps. He also mentioned that he is preparing a demo for later in the month to walk people through the basics of what the security work involves, aimed at those who rarely touch it.

On planning, we looked at the next two sprints. The principle was to leave slack. The last few sprints were packed to the brim, and every surprise caused a knock-on delay. Leaving a little room is not laziness; it is how plans survive contact with reality. We will state in the sprint notes how much room has been left, so that it is visible and nobody fills it by default.

One more thing for the diary: a reminder that the incident channel should be used only for incidents. Questions and chatter go elsewhere. It is a small discipline that makes the channel useful when it matters.

More in the next entry.

## Thursday and Friday

Two days rolled into one entry, because Thursday was mostly meetings and Friday was mostly cleaning up after them.

The triage pass on the scheduling backlog started on Thursday afternoon. Marlin Core's tickets were split into three piles by whoever happened to be reading them: do soon, park with a reason, close. It went faster than anyone predicted. The surprising part was how many items could be closed without anyone objecting; several referred to features that had since been replaced, and a few were duplicates of each other. By the end of the session we had a list that people could read without sighing. It still needs a second pass, but the shape is right.

A conversation worth recording came out of that: how to stop the backlog refilling. The answer we reached is boring and probably correct. New tickets need a clear description and a named requester, and anything that sits untouched for a long stretch gets a polite "still needed?" message. If there is no answer, it is closed. We will write that rule on the intake page so that it does not come as a shock.

Friday was devoted to the restore drill that I mentioned earlier. The result was mixed. The restore worked, but the written steps were out of date and two of them referred to a location that no longer exists. We fixed the document as we went, and the person who ran the drill added a note to say how long it took, which is useful to know. The lesson is the usual one: a backup you have not restored is a hope, not a plan.

Other items:

- The stale dashboard panel was removed. Nobody has complained, which says everything.
- The runbook "misc" page has been split into four proper pages. Titles were agreed in a couple of minutes, so I will take that as a sign they make sense.
- Callum Brennan asked whether the team could put together a short list of questions about security that people feel embarrassed to ask. The thinking is that these questions are often the most useful, and an anonymous box might help. We will try a simple form.
- The badge printer has labels again.

We discussed meeting load. Thursday had four meetings, and everyone agreed that this was too many for one day. A rule of thumb was suggested: no more than two recurring meetings on any day, unless there is a good reason. People will try to move things around, and we will see how far it gets. The first step is just to look at the calendar and notice where the clusters are.

I should also note that the quiet-hours idea was raised again, a block of the day where nobody schedules anything and no one expects a quick reply. It was well received, and we will propose a slot and trial it. The point is to protect time for work that needs concentration.

Next entry: preparations for the demo, plus anything left from the sprint review.

## Demo day and wrap-up

The demo took place at the end of the week, and I am writing this the same evening so that the details are accurate.

Attendance was better than anyone had guessed: 39 people attended the Callum Brennan demo. He had worried that a session about security would be a hard sell, but the room was full, and the questions afterwards ran on longer than the presentation. His approach was to avoid jargon and to start with things people can recognise from daily life, such as locking a screen, spotting a suspicious message and knowing whom to tell. He used a couple of invented examples, and that worked well because nobody felt singled out. The consensus was that it should be repeated, perhaps twice a year, aimed at newcomers.

What people said afterwards, in the corridor and in the feedback form:

- They liked that it was short and that it ended on time.
- They wanted a one-page summary to keep at their desk. Callum Brennan will write one, and we will keep it to what fits on a single side.
- Several asked how to report something suspicious without feeling foolish. The answer, which we agreed to put in writing, is that there is no such thing as a silly report.
- A couple of people asked for a version aimed at those who work from home, so that will go on the list.

A small point about logistics. The room was hot and the projector needed a restart, both of which are now familiar. Someone in facilities has been asked about the room; the projector remains a matter for the cupboard and the cable.

On the sprint side, the triage of the scheduling work is half done, and the team plans to finish it early next week. After that we will pick the next sprint's work from the cleaned list and, for the first time in a while, we should be choosing from a list we all believe. The principle of leaving slack was agreed, and it will be noted in the sprint plan.

Remaining chores for the diary:

1. Publish the corrected restore steps on the wiki, and link them from the runbook index.
2. Set up the anonymous form for security questions, and let people know where it is.
3. Rearrange recurring meetings so that no day has more than two.
4. Write the one-page security summary, then ask three colleagues to read it and say what is unclear.
5. Choose a quiet-hours slot and announce the trial.

Closing thought. This was a good week for the team in a modest way. Nothing broke, a couple of nagging problems got fixed, and a presentation that could have been dull turned out to be useful. I will try to remember weeks like this when the next fire arrives. Next entry will start with the sprint plan.
