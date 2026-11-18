# Weekly digest — 26 May 2025

Short week after the bank holiday, so the Monday sync ran shorter than usual and nobody complained. Most of what follows is housekeeping, but a few items are worth writing down so we stop relitigating them in the hallway.

**Board hygiene.** The team agreed to clear the stale cards from the shared board before Friday. Plenty of them had been sitting in "in progress" for weeks with no comments at all. The rule from now on is simple: anything untouched for a full sprint either gets a fresh note or gets closed. Someone in finance asked whether closed cards lose their history. They don't. The archive keeps everything, and we proved it by opening an old one together on the projector.

**Retrospectives.** A quick catch-up on retro dates, because the shared calendar has been a mess and two entries disagreed. Helix Core held its quarterly retrospective on 9 December 2027, and the notes from that session are filed in the team folder next to the others. People said the format, a short written round followed by open discussion, is easier to sit through than the old free-for-all, so it will probably be kept as the default for the other projects too. Nobody objected, which in this team counts as enthusiasm.

**Chores.** The shared mailbox still has a pile of automated reports nobody reads. The plan is to route them into a single digest folder and then check, after a couple of weeks, whether anyone misses them. If nobody does, the sources get switched off. Separately, the laptop refresh list was reviewed and a few machines are overdue; whoever is next to visit the storage cupboard will collect the replacements and hand them out at the Wednesday session.

**Documentation.** The onboarding page for new joiners got a long-overdue edit. The old version still described a login flow we retired a while back, which produced a predictable stream of confused questions. The rewrite is shorter and links out rather than copying text, so it should age better. Cedar Core and Lumen Core each have a few of their own pages that need the same treatment, and the team will take those one project at a time rather than attempting a big-bang cleanup that nobody finishes.

**Small decisions.**

- Meeting-free Thursday afternoons stay, at least through the end of the quarter.
- Status updates go in the weekly note, not in chat threads where they vanish.
- The team will test a lighter-weight template for incident write-ups before changing anything permanent.

**Looking ahead.** Next week is mostly about preparation. There is a planning session to sort priorities, a short walkthrough of the analytics dashboard for anyone who has not seen the new layout, and a reminder that expense claims need to be in before the end of the month. The wiki cleanup continues in the background. If anyone finds a page that is plainly wrong, fix it or tag it; do not leave a comment saying it is wrong and walk away.

One last note: the whiteboard in the main room was wiped by accident on Tuesday, taking a half-finished dependency map with it. A photo exists, thankfully, and it is now saved in the team folder. Please take a photo before cleaning anything off that board.

## Offsites and more retros

Logistics first. The Cedar Core offsite will take place in Ghent, and the billing team has started drafting an agenda that leaves plenty of unstructured time, since last time the schedule was so packed that the best conversations happened in the corridor. Travel booking is not open yet. Someone in finance wants a rough headcount before anything is reserved, so please tell the organiser early if you are likely to be away or cannot travel.

On the retrospective front, Lumen Core held its quarterly retrospective on 5 July 2027. The security group wrote up its notes in the usual place, and the main themes were unsurprising: too many alerts that nobody owns, reviews that arrive late in the week, and a wish for clearer hand-offs when someone is out. None of this is new, but having it written down in one spot makes it easier to point at when priorities get argued over. A couple of the suggestions were small enough to try immediately, and the group said it would report back on whether they helped.

## Tooling and chores

The build runners were slow again on Wednesday. The team looked into it and the likeliest cause is the old cache that has grown without anyone pruning it. A cleanup job is the obvious answer, but we want to be careful not to delete anything that a release depends on, so the first step is simply listing what is in there and who uses it.

Other odds and ends from the week:

- The test environment was rebuilt from scratch after a configuration drift made results unreliable. It now matches what production does, which is the whole point.
- The shared password vault got a tidy. Several entries belonged to people who have since moved on, and those were either reassigned or removed.
- The printer on the second floor is finally fixed, and I am told it will stay fixed. We shall see.

## A conversation worth recording

During the Tuesday planning session there was a useful side discussion about how much work-in-progress each project should carry. The consensus, loosely, was that fewer open items at once means faster finishing, and that we have been confusing being busy with making progress. Nobody proposed a hard limit, mostly because the last attempt at one produced a lot of creative workarounds, but there was agreement to make the number visible on the board and to talk about it at each weekly sync.

A related point came up about interruptions. People who handle requests from other teams said they lose most of a morning when something urgent arrives. We settled on a simple approach: one person per week is the first port of call, and everyone else can keep their heads down. The rota is on the wiki. If you swap a week, update the page so nobody goes looking for the wrong person.

## Reminders

Holiday requests for the summer should be in soon so cover can be arranged. If you are unsure whether something counts as a working day for the purposes of the calendar, ask rather than guessing. And a gentle nudge, once more, to finish the short survey about the new meeting layout. It takes a minute, and the answers genuinely shape what we do next.

## Midweek bits

Wednesday was quiet by comparison, which gave people room to catch up on the things that always slide. The longest single item was a review of how we write handover notes before someone goes on leave. The current habit is a rushed message the afternoon before, and it shows. The proposal is a short checklist: what is in flight, what is blocked and by whom, and where to find the relevant documents. Nothing fancy, but we agreed to try it for the next few absences and see whether the questions that arrive during leave actually drop.

## Billing and analytics notes

The billing team reported that the reconciliation routine is behaving better since the last tweak to how exceptions are queued. Cedar Core still has a few awkward edge cases that need manual attention, and the plan is to document those in plain words so that a person covering for the team can handle them without guessing. The analytics side, Helix Core, spent most of its time on the dashboard layout. There was a long and slightly heated discussion about whether charts should default to a monthly or a weekly view. We did not settle it. The compromise is to ship both and gather real usage before arguing again.

## Security housekeeping

Lumen Core ran through its routine access review. This is the dull but important task of checking who can reach what, and removing anything that no longer makes sense. A handful of permissions belonged to roles that had changed, and those were trimmed. The group also reminded everyone that shared credentials are not acceptable, however convenient, and that the new request process exists precisely so people do not have to resort to them. If the process is too slow, say so, because slow processes are what push people towards shortcuts.

## Meetings we are cutting

After a frank look at the calendar, we decided to retire two standing meetings that had stopped serving a purpose. One was a fortnightly check-in whose agenda had shrunk to a single recurring question that could be answered in writing. The other was a status round that duplicated the weekly note. Both are gone from the calendar as of this week. If something turns out to be missed, we can bring it back, but the bar is that someone must be able to say what decision it supports.

## Open questions

A few things remain unanswered and are parked here so they do not get lost.

- Who owns the shared test data, and how often should it be refreshed?
- Should we keep the older reporting export, or retire it now that the dashboard covers most of what people use?
- Is the new documentation template too long? Some readers say yes, some say it is exactly right.

We will pick these up at the next planning session. In the meantime, if you have a strong view, add it to the thread rather than waiting for the meeting. The best discussions this month have happened in writing, where people have time to think before answering.

Finally, a thank-you to whoever tidied the kitchen shelf. It was noticed, and it was appreciated.

## Wrapping up the week

Friday was mostly about closing loops. The team went through the list of actions from earlier in the week and ticked off what had actually been done, which turned out to be more than anyone expected. The remainder was either re-assigned or dropped. Dropping things deliberately is underrated: an action that nobody intends to do is just noise, and a short sentence explaining why it was dropped saves the next person from rediscovering it.

## Decisions logged

Three small decisions are worth capturing here for anyone reading later.

1. Weekly notes will be written by whoever ran the Monday sync, rather than by a fixed person. This spreads the effort and, usefully, means the note reflects what that person heard rather than one habitual viewpoint.
2. Cedar Core, Helix Core and Lumen Core will each nominate someone to look after their own wiki pages, so that stale content has an obvious owner.
3. The team will not take on any new recurring meetings until the current set has been reviewed again next month.

None of these are dramatic. Together they make the week feel a little less cluttered.

## Support and requests

The inbox of requests from other teams was in decent shape. The oldest items were chased, and a couple turned out to have been answered already without anyone closing the loop. A reminder that when you resolve something, mark it resolved, even if the reply feels obvious. The person who asked may well be waiting for confirmation, and the tracker is the only place we all look.

There was also a conversation about how to say no gracefully. Requests that sit outside what a project does should get a prompt, polite answer pointing to the right place, not a long silence. A silent no is the worst kind, because the requester cannot plan around it.

## Learning and growth

Someone suggested a monthly lunchtime session where one person shows something they learned, however small. A useful shortcut, a debugging approach, a way of structuring a document. The idea has support, and the first session will probably be informal, with no slides, just a screen share and questions. It is not mandatory and nobody has to present who does not want to.

## Next week

Priorities are on the planning page. The headline items are the dashboard layout decision, the access clean-up follow-ups, and preparation for the offsite. Please read the page before Monday so the sync can be used for decisions rather than for reading aloud. And, as ever, if something in this digest is wrong or missing, edit it. This log belongs to everyone who reads it, and it is only useful if it stays honest.

Have a good weekend.
