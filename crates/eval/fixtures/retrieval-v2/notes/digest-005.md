# Planning notes — 29 May 2025

Thursday planning session, held in the big room because the small one had a broken projector. These notes capture what we decided and what we put off.

## Setting the scene

The point of the session was to line up the next stretch of work across the three project groups and see where they bump into each other. Billing, maintenance and telemetry rarely collide on a daily basis, but when they do the results are messy, and we would rather find the overlaps on paper than in production.

We began with a quick round of "what are you worried about". The answers were refreshingly specific. Cedar Core, which handles billing, is concerned about the end-of-month crunch and wants fewer interruptions in the last days of the period. Tern Core, the telemetry project, would like a quieter stretch to rework how its data is collected and tidy up the naming. And Nimbus Core, responsible for maintenance, wants predictable windows for upkeep work, since surprise requests tend to push planned jobs aside.

### Training

On the learning side, Nimbus Core completed 8 hours of training this term, which is lower than the group wanted. The explanation was simple: upkeep work always seems more urgent than a course, so the course gets pushed. The team agreed that was not good enough and decided to reserve a regular, protected slot each week, even if it is short. A small, steady amount is more realistic than a heroic block that gets cancelled. Someone suggested pairing the training with real tasks so it feels less like a separate chore, and that went on the list.

### Plans for the coming weeks

- Agree upkeep windows with the other two groups and put them in the shared calendar.
- Reduce late-period interruptions to billing by routing questions through a single channel.
- Give telemetry a clear week for its naming cleanup, with no new feature requests during it.
- Keep a short list of dependencies between the groups so surprises are visible early.

### Small decisions

We settled on a shared glossary for terms that mean different things to different groups. "Done" apparently means something else to everyone. A plain page with a few agreed words will be started this week and edited by whoever spots a gap.

We also decided that planning sessions end with each group stating, in one sentence, the thing it will deliver first. That keeps meetings from drifting into general talk, and gives everyone something specific to ask about next week.

### Next steps

The glossary draft, the shared calendar windows and the dependency list are the three concrete deliverables from this part of the session. The rest of the notes cover the second half of the discussion and the decisions that came after it.

## Dependencies and risks

After a short break we moved from worries to mapping, which is where the session got interesting. We drew boxes on the whiteboard for each group and then drew arrows wherever one group depends on another. By the end the board looked like a bowl of noodles, but a few arrows stood out as thicker than the rest.

The biggest one runs from telemetry to billing. Billing relies on data that telemetry collects, and when the collection format changes, billing finds out the hard way. Cedar Core asked for advance notice of any change, with a short description of what moves and what stays the same. Tern Core agreed in principle, with the proviso that "advance" has to be realistic, since some changes are only understood once the work starts. A compromise: a heads-up as soon as a change is likely, followed by a confirmation when it is certain.

The second thick arrow runs from maintenance to everything else. When upkeep work causes a brief outage, the other groups feel it. Nimbus Core accepted that its windows need to be better advertised and promised a one-line notice beforehand each time. People felt this would solve most of the complaints, since the pain is mostly surprise rather than the interruption itself.

### Risks we named

- Staff holidays overlapping in the same weeks, leaving a group thin.
- A hand-off between groups that depends on one person's memory.
- Planned work crowding out routine work until the routine work becomes urgent.
- Documentation that is accurate on the day it is written and wrong a month later.

For each, someone was assigned to watch it and report back at the next planning meeting. Nobody was asked to solve any of them, only to keep an eye out.

### Process tweaks

We want the weekly check-in between groups to be shorter and sharper. The format is now three questions: what is about to change, what do you need from us, and what is at risk. Anything else can go elsewhere. If a check-in runs over, the chair is allowed to stop it.

There was also a discussion about estimating. A few people felt that estimates were being treated as promises. We agreed to give ranges instead of single figures and to say how confident we are. It is a small change in wording that might change a lot of conversations.

### Side notes

The coffee budget came up, briefly and with feeling. It was decided to keep it as is. The old projector in the small room will be reported again, since it has now broken at three separate meetings.

Overall the mood was constructive, with a clear sense that mapping dependencies had been worth the effort. The final part of these notes records the remaining commitments and the dates that matter.

## Commitments and calendar

The last stretch of the session was about turning talk into commitments, and making sure each one has a place in the calendar.

Cedar Core completed 14 hours of training this term, and the group felt that was a solid result given how busy the billing cycle has been. The plan is to keep the same pace, with a short session booked into the quiet days after each cycle closes, since that is when people actually have headspace. Billing will also share what it learned with the other groups in a brief write-up, so the benefit spreads beyond the people who attended.

On the review side, Tern Core will hold its quarterly retrospective on 11 November 2026. That is a long way off, but the telemetry group likes to fix its dates early so other groups can plan around them. They asked that any dependency questions be brought to that session rather than raised ad hoc, with a shared list kept in the meantime so nothing is lost. Everyone agreed the date was sensible and put it in their calendars on the spot.

### What each group commits to

- Billing: the late-period interruption plan, and a short note on its training for the others.
- Maintenance: the advertised upkeep windows and the one-line notice before each one.
- Telemetry: the naming cleanup week, plus advance warning of changes that might touch billing.

### Shared tasks

The glossary draft goes to everyone for comment. The dependency list is kept in one shared page, and each group owns its own rows. A short review of both happens at the next planning meeting, where we will check whether they are being used or just admired.

### Things we deliberately did not decide

We left the question of a combined on-call arrangement open. It seems attractive, but there was not enough time to think through the effects on each group. It goes on the agenda for the next session with a named person gathering the facts first. We also parked the question of whether to merge two of the weekly meetings; people were split and no one wanted to force it.

### Wrap-up

By the end people looked tired but satisfied. The whiteboard noodles were photographed and filed, with the thick arrows circled. One person remarked that it was the first planning session in a while where nobody left feeling they had been talked at, and I would like to repeat that.

Next planning meeting is already in the diary. Bring the glossary comments, the dependency rows and a more reliable projector, if anyone can arrange one.
