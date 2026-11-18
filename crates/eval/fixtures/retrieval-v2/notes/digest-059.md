# Journal — 29 September 2025

Journal for a damp, slow-moving week. The big news is that there is no big news, which after the summer is a relief.

Monday's meeting was mostly about telemetry. Marcus Haddad led it and began with a confession: the signal-to-noise ratio in the main views has got worse, because every incident since the spring has left a new chart behind. His plan is to hold a "chart amnesty", where anyone can nominate a chart for retirement and it goes unless someone speaks up within a few days. People liked the framing. It is a lot easier to remove things when removal is the default and keeping needs a reason.

He also mentioned that some of the alert thresholds were set during a noisy period and never revisited. Rather than adjusting each one by hand, he wants to look at how often each has fired and whether anyone acted. The ones with a long record of being ignored will be loosened or removed. We discussed whether to involve other teams, since a few alerts feed their work. The answer was yes, with a clear note saying what is changing and when, so nobody is surprised.

Scheduling came next. Marlin Core is the project that handles it, and it had a calm week. The calendar for the next month is mostly settled, apart from a couple of slots that depend on decisions elsewhere. There was a short discussion about how to display uncertainty: some people prefer to leave unconfirmed items off the calendar, others want them shown as tentative. The compromise was a simple marker on anything provisional, with a note saying what it is waiting for. That seems to give both camps what they need.

A few smaller notes:

- The shared drive has a new folder structure, and the old one now carries a banner pointing to the new locations. If you cannot find something, ask rather than create a duplicate.
- The weekly written updates are working. Most are short, and several are actually interesting.
- Someone suggested we try a rotating "question of the week" at the start of Monday meetings, to warm things up. Opinions were mixed, so we will try it for a month and then decide.
- The cupboard in the meeting room is now sorted, and the labels are legible. We found a surprising number of spare adaptors.

On the people front, Marcus has been thinking about how to share what he learns about the monitoring side so that nobody becomes a single point of failure. He plans to write short notes after each significant change, to be filed alongside the relevant chart. The notes need not be polished, only readable. Others in the group offered to review the first few, mainly to check they make sense to someone outside telemetry.

An item that deserves a mention: we agreed not to schedule major changes in the week before holidays, unless there is a clear reason. The reasoning is that if something breaks, we want most of the team present to deal with it. It sounds obvious, but we have broken this rule before, and writing it down gives us something to point at.

Actions for next time: run the chart amnesty, review alert history, add provisional markers to the calendar, and start the post-change notes. I will also remind everyone about the question of the week experiment, in case it has slipped from memory.

## Amnesty results and offsite news

The chart amnesty ran for the agreed few days and the results were pleasantly undramatic. A good number of charts were nominated and only a couple were defended. One was saved because a colleague used it weekly without telling anyone, which is a nice reminder that usage is not always visible. The rest have been moved to an archive view, so they are out of the way but not lost. Marcus Haddad is pleased and wants to repeat the exercise every quarter.

Training came up during the conversation about how people spend their development time. Marcus Haddad completed 47 hours of training this term, mostly on observability techniques, and he shared a few takeaways. The most practical was a way of describing what a chart is meant to show before building it, which would have prevented a lot of our clutter. He has offered to run a short session, and a few people have put their names down. The more general discussion was about whether we should keep a loose record of what each of us has learned. The consensus was to mention it in the weekly update, in a sentence, and leave it there.

The offsite plans have taken shape. The Marlin Core offsite will take place in Cork. It is early days, and the first conversation was about what we want out of it. The ideas so far include a look at how scheduling decisions get made, a session on keeping the calendar honest, and some time for unrelated conversation. A couple of people mentioned they would like to include a visit to something local, and that is now on the list. The group agreed to keep the schedule light and to leave the evenings free.

Practical points from the same meeting:

- Someone in finance will check what is needed for the booking process, and we will plan around their answer.
- Availability needs confirming, and anyone with constraints should say so soon.
- We discussed travel on the same day as the first session and decided it is better to arrive the evening before, so nobody starts the day frazzled.
- A small budget has been requested for a shared meal. It will be handled by finance.

Meanwhile, the alert review made progress. The history shows a long tail of alerts that fired many times and led to nothing, and these are being loosened one by one. For each change, Marcus is posting a short note so other teams are aware. So far nobody has objected, and a couple of people have said thanks, which is not something you expect for alert changes.

The question of the week experiment began with a quick question about favourite lunch spots. It was received with polite enthusiasm. We will see whether it survives the month.

On the calendar, the provisional markers are now in use, and they seem to help. People can see at a glance what is certain and what is waiting, and the note about what it is waiting for saves a lot of messages. One of us said it was the first time the calendar had felt honest.

Remaining actions: confirm availability for the offsite, schedule the observability session, keep trimming alert thresholds, and plan the next amnesty. A reminder that the major-change freeze before holidays starts soon, so anything risky should go in this week or wait.
