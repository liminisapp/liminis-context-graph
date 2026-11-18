# Sync notes — 5 May 2025

Monday sync, held in the small room because the big one was taken by a training session. Notes are in the order things came up, which is not the order they were on the agenda. That is normal.

## Opening round

We began with the usual quick pass around the table: what landed last week, what is stuck, what needs someone else's attention. It was a gentle start. Most people reported steady progress, a couple admitted to a week lost to meetings, and one person confessed to having spent an entire afternoon on a bug that turned out to be a typo in a configuration file. We all nodded with great sympathy, because every one of us has done the same.

The bank holiday meant a short week for some, so the rota for coverage was confirmed at the start. Nothing complicated: whoever is in covers the shared inbox and the urgent channel, and anything else waits. The rota is pinned in the usual place if you need to check.

## Routing and incidents

The routing side had the first real item. Quill Core logged 41 incidents during the quarter, and we spent a good while looking at what sat behind that number. The conversation was constructive rather than defensive. Many of the incidents shared a small set of causes, mostly around how requests get passed between steps, and a few were repeat visits from the same problem that had been patched rather than fixed. The proposal was to pick the two most repeated causes and give them proper attention in the coming weeks, instead of trying to chase everything at once. There was general agreement. Someone suggested that a short write-up of each repeated cause would help newcomers recognise it faster, and that was added to the list.

We also talked about how incidents are recorded. The wording tends to vary a lot from one author to the next, which makes counting and comparing harder than it should be. A light template was floated, with three fields only: what was seen, what was done, and what to watch for. No one wanted a heavyweight form, and this is not one.

## Support backlog

Next came support. The Juniper Core backlog stood at 28 open tickets at the end of the sprint. That is manageable, and the mood about it was fairly relaxed, although we noted that a few of the tickets had been open far longer than the rest and deserved a look. The plan is to go through the oldest ones first and either close them with a clear note or escalate them to someone who can act. There was a short discussion about whether tickets should be closed automatically after a period of silence. Opinions split. In the end we agreed not to automate anything yet, but to review stale tickets by hand each Friday for a month and see how many there really are.

One practical point: support requests that arrive through side channels, such as a quick message to a person rather than a ticket, do not show up in the count. A reminder was issued, politely, to point people back to the proper queue. This is not about bureaucracy. It is about being able to see the real workload.

## Housekeeping before moving on

- The shared calendar for the month has been cleaned up, with duplicates removed.
- Laptop updates are due this week, so please do not postpone them again.
- The kitchen has a new rota and a new, stricter policy on abandoned mugs.

More follows in the next section, including the retrospective discussion and a few planning notes.

## Retrospectives and planning

The second half of the sync turned to how the different groups run their retrospectives, since a few people had asked whether there was a common approach worth adopting.

On the maintenance side, Nimbus Core held its quarterly retrospective on 5 October 2026. That was mentioned as an example of a rhythm that seems to work: a fixed point each quarter, a short preparation period in which everyone adds their thoughts in advance, and a meeting that is mostly about picking a few things to change rather than recounting everything that happened. The people who attended spoke well of it, and the notes from it are easy to follow because they stick to a plain structure. Others in the group said they would like to borrow the format, perhaps with a few tweaks to suit smaller teams.

There was a discussion about what a good retrospective actually produces. The answer we reached was that it should produce a small number of concrete changes, each with an owner and a rough date, and nothing else. A retrospective that generates a long list of feelings but no changes may be pleasant but it does not move anything. Several of us admitted to having sat through exactly that kind of session. One person proposed a simple test: if a month later nobody can name what changed because of the last retrospective, it was not worth the time.

On facilitation, the suggestion was to rotate the role. When the same person runs it every time, the discussion tends to follow their concerns. Rotating also spreads the skill around, which is useful when someone is away. We agreed to try that for the next cycle and see whether anyone hates it.

## Planning for the coming weeks

A few planning notes came out of the conversation:

1. The two most repeated causes of routing incidents get a named owner each, and a short update at the next sync.
2. Support will review its oldest tickets on a fixed day each week, for a trial period of a month.
3. The incident template gets drafted by whoever feels most strongly about it, which in practice means whoever complained loudest. Volunteers are welcome.
4. Maintenance will share its retrospective notes format on the wiki so that others can copy it.

There was a fair amount of banter during this part, mostly about the unfairness of being the person who complains loudest and then ends up with the task. It was taken in good humour. It was also pointed out that the person who has the strongest opinion about a template is usually the person most likely to make a decent one.

## A few loose items

The question of working hours over the summer came up briefly and was parked. Nobody is ready to decide, and there is no deadline. The only commitment is that we will pick it up again before the end of the month so that people can plan holidays.

The new shared folder structure was also mentioned. It is roughly in place, with a clear top level and fewer nested folders than before. If you cannot find something, ask in the channel rather than creating a duplicate, as duplicates are how we ended up here in the first place.

The final part of the log covers the wrap-up, the carry-over items and some personal observations about how the meeting went.

## Wrap-up

By the time we reached the end of the agenda, the room had warmed up considerably and someone opened a window. A few last items before we dispersed.

**Actions agreed, in plain terms.**
- Name an owner for each of the repeated routing causes, and let the sync know by next Monday.
- Draft the incident template, three fields, one page, no decoration.
- Start the Friday review of stale support tickets, and record how many turn up each week so we can judge whether the effort is justified.
- Share the retrospective format on the wiki.
- Rotate the facilitator role at the next retrospective, and ask afterwards whether it helped.

**What went well in the meeting.** It stayed on topic for most of the hour, which is an achievement. Everyone contributed, including a couple of people who tend to stay quiet in larger groups. The small room helped, since nobody could hide behind a screen, and the whiteboard meant the actions were visible while we were agreeing them rather than being reconstructed afterwards.

**What could be better.** We lost about ten minutes in the middle to a tangent about naming conventions, which, while interesting, belongs in its own session. We also did not leave enough time for the support discussion to reach a decision. The compromise to review by hand each Friday is sensible, but it was reached under time pressure, and it might be worth revisiting after the trial month with a calmer head.

**Reflection.** There is a pattern worth noting across the three areas discussed today: routing, support and maintenance. In each case, the useful move was to make the work countable and then look at the count with other people. The numbers are never the whole story, and sometimes they are misleading, but having them on the table gives the conversation a shape. Without them, we tend to argue from impressions. With them, we argue from the same impression, which is a considerable improvement.

**Notes on tone.** Several people commented, after the meeting, that the discussion felt less like a status report and more like a conversation among people who want the same outcome. That is probably down to the small room and the lack of a slide deck. We should consider holding more of these without slides.

**Reminders.**
- Laptop updates, again. Please.
- The shared inbox rota for the holiday week is on the usual page.
- If you took a mug home, please bring it back. The kitchen is running low and the new policy is not a joke.

Next sync is the same time next week, same room if it is free. Bring your own pen, since the whiteboard markers are down to the last one and it is not in good shape.

Thanks to all for a productive hour, and to the person who brought biscuits, whoever you are.
