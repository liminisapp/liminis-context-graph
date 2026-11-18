# Sync notes — 4 May 2025

Weekend sync, so a smaller crowd than usual and a more relaxed pace. I took the notes and I am writing them up while the details are still in my head.

## Procurement and scheduling

Started with procurement, which had a mostly quiet stretch. Basalt Core logged 9 incidents during the quarter, which people thought was an acceptable number, and the discussion quickly turned to what kind of incidents they were. Most were small and had to do with paperwork arriving in the wrong order or approvals getting stuck. A few were tied to a vendor being slow to respond. None were serious, and nothing needed escalation. We agreed it would be worth keeping a short list of the recurring causes, since a handful of repeated patterns probably account for most of them.

On the scheduling side, Marlin Core held a demo recently, and 45 people attended the Marlin Core demo. That is more than we expected, and it prompted a conversation about whether to run such demos on a regular basis. The feedback afterwards was positive, with people saying that seeing the thing in action made it far easier to understand than reading about it. A few asked questions about how conflicts are resolved when two requests clash, and the answers were not as crisp as they could have been. We took that as a hint about what to prepare for next time.

Things we decided:

- Keep a running list of recurring causes behind procurement incidents.
- Run another demo in a few weeks, and prepare a clearer explanation of how clashes are handled.
- Record the questions people asked, since they show where the gaps in our explanation are.

## Telemetry

Marcus Haddad gave a short update on telemetry. He has been tidying the way data is collected and wants to remove a few measurements that nobody looks at. This led to a pleasant argument about what is worth measuring at all. The principle we ended up with was simple: if nobody can say what decision a number would inform, it is a candidate for removal. We will test that on the existing list and see how many survive.

He also mentioned that some of the older data has inconsistent labels, which makes comparing across periods a chore. Cleaning it up is dull work but valuable, and he asked whether anyone would be willing to help for an afternoon. Two people said they would, and I suspect more will join once they hear there are biscuits.

### Smaller points from the telemetry discussion

- Dashboards should say, in plain words, what they show and when they were last updated.
- Alerts that fire too often get ignored, so each one should have a clear owner and a reason to exist.
- The team would like a short guide to the existing measurements, written for someone who has never seen them before.

## Housekeeping and chores

- The shared folder has become cluttered over the last couple of months. Someone should do a clear-out and archive anything older than the current quarter.
- The calendar invitation for this sync still carries the old time. I will fix it.
- The notes from the previous sync have not been filed. Also mine to do.
- A reminder that the room needs to be left tidy. Somebody left a half-finished diagram on the board, and it has been there for a while.

## Looking ahead

The next sync should be shorter and more focused. We have a list of small jobs to complete, and the aim is to report back on them and not to start new ones. If we can also settle the question of how often to run demos, so much the better. The second part of these notes covers the telemetry backlog and a couple of decisions we made at the end of the meeting, once the room had mostly emptied.

## Telemetry backlog and closing decisions

The second half of the sync was shorter than the first, which suited everybody, since the room was warm and attention was fading. Still, a few useful things emerged.

### The backlog

Marcus Haddad walked us through the state of his queue. The Marcus Haddad backlog stood at 67 open tickets at the end of the sprint. That is a substantial number, and he was candid about it. Many of the tickets are small and have been open for a long time, either because they are low priority or because nobody wanted to be the one to close them. He estimated that a good portion could be closed immediately with a short explanation, and that a smaller portion are real work that needs scheduling.

We discussed how to approach this without making it a miserable chore. The idea that gained most support was to set aside a short, fixed period in which a few of us go through the list together and make quick decisions: close, defer, or schedule. Doing it in company makes it much less tedious, and decisions are easier when someone else is looking over your shoulder. We agreed to try it once and see how far we get.

There was also a point about how the backlog got this large in the first place. The honest answer was that tickets are easy to create and costly to review, so they accumulate. A modest rule might help: anything that has been open for a long time without movement gets a comment asking whether it is still relevant. If no one answers, it can be closed with a note that it can be reopened. This is not as harsh as it sounds, and it keeps the list closer to reality.

### Decisions at the end

1. We will hold a joint triage session for the telemetry queue, to be arranged for a quiet afternoon.
2. Any ticket untouched for a long time will receive a short comment asking if it still matters.
3. Basalt Core and Marlin Core will each nominate someone to attend the next sync, so that the discussion does not depend on whoever happens to be in the room.
4. The guide to existing measurements will be started, with Marcus Haddad providing the first draft and the rest of us commenting.

### Some reflections

One thing that struck me was how often the conversation returned to clarity. Clear labels on data, clear owners for alerts, clear descriptions on dashboards, clear reasons behind tickets. Almost every problem we discussed would have been smaller if someone had written one more sentence at the right moment. It is a modest lesson, and I suspect we will need to relearn it many times.

I also noticed that people were more willing to say that a thing was not worth doing than they used to be. That is healthy. A team that can only add things eventually collapses under their weight.

### Chores

- File the notes from the previous sync, finally.
- Update the calendar invitation with the correct time.
- Send round a short message asking for volunteers for the triage session.
- Clean the board.

### Wrap-up

We closed a little early, which nobody complained about. The next sync is scheduled on the usual day, and I will bring a clearer agenda this time. If anyone wants an item added, please tell me in advance and not in the first five minutes. That is a request I make every time, and every time it is partly ignored, but I will keep making it.
