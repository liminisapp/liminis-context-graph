# Planning notes — 28 June 2026

Sunday-evening planning notes, written ahead of a week that looks busier than usual. I like doing this the night before: the calendar is still blank enough that I can see what ought to happen first.

The headline from the last few days is the internal audit. Marlin Core scored 66 out of 100 in the internal audit. Nobody on the scheduling side was surprised, since the weak areas were ones they already knew about, mostly around record-keeping and the way changes to shared timetables get approved. Still, a number on paper concentrates the mind. The reaction in the room was sensible rather than defensive: pick the three or four items that dragged the score down most, fix those first, and leave the cosmetic ones for later. It helps that the audit comments were specific, so each finding points at something concrete, not at a general feeling of unease.

Some thoughts on how to approach the fixes. Record-keeping problems are boring but cheap to solve, so those can go first and give a quick, visible improvement. The approval process for timetable changes is harder, because it involves more than one group, and it needs a conversation before any paperwork. I would like to see a short written description of the current process, drawn from how people actually behave rather than how the old document says they should. Comparing the two will probably be educational and slightly embarrassing.

Other things on the list for the coming days:

- Reconfirm who is covering the on-call slots, since a couple of swaps have been agreed in conversation only. Get them into writing.
- Look at the analytics side with Helix Core and decide whether the weekly summary is still serving anyone. Several people said they stopped reading it a while ago.
- Ask Marcus Haddad whether the telemetry dashboards have settled down after the last round of changes, or whether we should expect more tinkering.
- Clean out the shared calendar of recurring events that no longer have a purpose.

There was a short conversation about workload. The sense is that the next fortnight is heavier than it looks on paper, partly because several people are out at various points and partly because the audit follow-ups will land on top of everything else. I suggested that we agree in advance what can slip, so that when pressure arrives we are choosing rather than panicking. The shortlist of things that can wait: the wiki tidy, the glossary additions and the laptop comparison. Everything else stays.

Finally, a small personal resolution, which I will state publicly so that it counts. I will write my updates in the log by the morning of the sync, not during it. Several of you have been doing this for ages, and it shows.

## Monday catch-up

Quick catch-up entry from Monday. Things moved faster than expected over the weekend, and I want to keep a record of what was agreed before it blurs.

The audit follow-up was the first item. A small group met in the morning to split the findings into three piles: do now, do soon, and do when convenient. The scheduling project, Marlin Core, took most of the do-now pile, which is fair since it was their audit. The ones that touch shared systems were passed to the groups that own those systems, with a short note saying what the finding was and why it mattered. I think passing them over with context is important. Nobody likes receiving a bare list of problems with no explanation.

Next, the weekly summary from analytics. Helix Core said they would happily stop producing it if nobody is reading it, but they would like to be sure first. The approach we agreed is to announce that it is going to stop at the end of the month and see who complains. If nobody does, it goes. If a few people do, we ask them what they actually use it for and shrink the summary to that. It is a gentle way of testing demand without a survey that nobody would fill in.

A chore that has been hanging around: the shared calendar. Someone went through it and removed a number of recurring events that had outlived their purpose. One of them turned out to be a meeting that had been cancelled for a long time and still occupied a slot every week. Freed-up time was noticeable, particularly on Wednesdays.

On telemetry, Marcus Haddad reports that the dashboards are largely stable. A couple of panels still behave oddly at certain hours, and he suspects a timing issue in how the data is collected. He is not going to chase it immediately, because the cost of the odd readings is low and he has other things to do. That seems like the right call. Not everything odd needs fixing today.

Some notes on how the week ought to run:

- Written updates before the sync, as usual.
- Friday merges need a second look. Several people mentioned the restriction has been helpful and they would like to keep it.
- The on-call swaps get recorded in writing, no exceptions.

And one cultural observation. In the audit discussion nobody tried to assign blame, which is not always the case. I think the specific, factual wording of the findings helped. It is easier to talk about a process than about a person. If we can keep that tone through the follow-up work, we will get through the list faster and in a better mood.

The next entry will cover what happened with the first batch of fixes.

## Quarter in review

End-of-quarter entry, a bit overdue. The conversation was a good one and I do not want the details lost.

Incident numbers first, because they always come up. Helix Core logged 47 incidents during the quarter. The analytics team's own reading is that this is mostly a function of how much new reporting they shipped, since each new report brought a handful of teething problems before it settled. They would rather have the count and the learning than hide the problems. Someone asked whether any of the incidents were repeats of an earlier one, and the answer was that a few were, which is precisely why they plan to add a short root-cause note to each one that is closed. Counting is easy; understanding is the part worth doing.

The telemetry side held a demo last week for people outside the group, and I want to record how it went. 40 people attended the Marcus Haddad demo, which was a respectable crowd for a talk about dashboards and alert tuning, normally not a headline draw. He kept it practical: here is what we used to page on, here is what we page on now, here is why. A few attendees said it was the first time the reasoning had been explained in plain terms, and one person left a comment asking for the slides, which he said did not exist because he had worked from a short list of notes. He promised to turn the notes into a page in the wiki.

Questions from the floor were thoughtful. The main one was how to decide whether an alert is worth keeping. His answer was a simple test: if the alert fired and nobody did anything, it is a candidate for removal; if it fired and someone acted, keep it; if it never fires, check that it still can. I wrote that down because it is the most useful rule of thumb I have heard in a while.

Other business:

- The on-call rota for the coming quarter needs filling. Please confirm your slots or tell the chair about conflicts.
- The audit follow-ups are progressing, with the simplest items already closed.
- A request to retire a few old dashboards has been logged. We will check for users before turning them off.
- The meeting-free morning stays, with a review next month.

A short word on morale. People are tired but in decent spirits. A few mentioned that the quarter felt long, and that a lighter week at the start of the next would be welcome. I think we should plan for exactly that: no big new commitments in the first few days, just tidying, catching up and getting the board in shape.

Next entry will look ahead to the coming quarter's priorities.

## Next quarter's priorities

Looking ahead now. This entry sets out what the group wants to focus on, with the reasoning, so that anyone joining the conversation late can see how we got here.

The first priority is finishing the audit follow-ups for Marlin Core. Most of the quick wins are done, and what remains is the harder material about how changes to shared timetables get approved. We agreed to write down the real process first, compare it with the official one, and only then decide what to change. A description of actual behaviour, even a messy one, beats a tidy ideal that nobody follows. The group would like this finished well before the next audit, not scrambled together in the final week.

Second is analytics. Helix Core will wind down the weekly summary unless people object, and put the time saved into a smaller number of reports that people actually use. There was general support. One suggestion that I liked is that each report should name who it is for and what decision it helps with. If neither can be stated, the report is a candidate for retirement.

Third, telemetry. Marcus Haddad wants to publish the alert rules of thumb he described at the demo as a short wiki page and keep it up to date as the dashboards evolve. He also wants to run a short review every month where on-call people say which alerts were useful and which were noise. The aim is a steady habit and not a big push, since the big pushes tend to fade.

A few supporting items round out the list:

- Keep the meeting-free morning, and measure whether it is actually helping by asking people at the end of each month.
- Continue the habit of writing updates into the log ahead of the sync.
- Keep Friday merges to a minimum and make sure someone else looks at anything that must go out.
- Run the restore test that has been put off, and write up the result honestly.

Thoughts on capacity. We discussed leaving a deliberate margin this quarter. The previous one was packed and the cost was fatigue and a lot of half-finished work. The plan is to commit to fewer things and finish them, then take on extra items if there is room. It is a small cultural shift, from proving how much we can carry to proving that we can finish what we start.

On a lighter note, the kitchen has been restocked, the dishwasher noise turned out to be a spoon, and the plants have survived another week. Small victories.

That closes this log for the quarter. Whoever opens the next one, please begin with the actions and keep the rest short.
