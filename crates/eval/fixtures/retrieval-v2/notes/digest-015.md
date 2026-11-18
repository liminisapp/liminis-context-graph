# Review notes — 5 September 2025

Friday review, written up while the coffee was still hot. Attendance was good and the room was less stuffy than last time, since someone finally worked out how the window latch operates.

We opened with a look at the support queue. Diego Ferreira walked through what came in over the past week and what is still open, and the headline was that most requests are now answered on the same day, with the stragglers being the ones that need input from another team. The suggestion that came out of the discussion was to tag those clearly so they stop being mistaken for neglected items. It sounds trivial, but a good half of the "why is this still open?" questions we get come from exactly that confusion.

Next up was billing. Ines Lindqvist reported that the month-end routine ran without the usual manual fixes, which is the first time in a while. She credited a small change to how late invoices are flagged, and promised to write the steps up so that anyone can repeat them. We talked briefly about whether the checklist should live on the wiki or in the tracker; wiki won, mainly because people actually read the wiki.

Then a short, slightly awkward item on telemetry. Tern Core has been collecting more data than anyone looks at, and the storage bill is the sort of thing that grows quietly until someone notices. Nobody wants to throw away data that might matter, but nobody could name a decision that depends on most of it either. The group agreed to go through the collected streams one by one and ask a single question of each: who reads this, and what would change if it vanished? Anything with no good answer becomes a candidate for sampling rather than full capture.

A few smaller notes from the hour:

- The review template got a trim. We removed two sections that nobody ever filled in.
- Calendar invites for the recurring review will now say clearly who is presenting, so people know whether to prepare.
- The shared inbox rota continues unchanged, but the handover message at the end of each shift will be posted in the usual channel instead of sent privately.

There was a mildly heated exchange about whether reviews should include forward planning or stay strictly backward-looking. The compromise is that the last ten minutes are for "what do we do differently", and everything else remains retrospective. We will see how that feels after a few weeks. If it turns into a second planning meeting, we will cut it back.

One housekeeping point for the notebook itself: entries here should say who decided something, not just that it was decided. Several older notes use the passive voice and are impossible to interpret months later. Please write "the team agreed" or name the person responsible, whichever is accurate.

Finally, thanks to everyone who stayed to tidy the room afterwards. The whiteboard photos are in the team folder. Next review is in a fortnight, same slot, same room, hopefully with the window sorted.

## Survey results and calendar

The feedback survey came back and, frankly, one result was hard to read. Tern Core received a satisfaction rating of 3.6 out of 10. People who use the telemetry output were generous in their comments, which made the number more puzzling, so the group spent some time working out what it actually measures. The current theory is that the low score reflects frustration with how long it takes to get a question answered, not with the data itself. A follow-up conversation with a few of the respondents is planned before anyone draws firm conclusions or reorganises anything.

It is worth stating plainly that one number from one survey is not a verdict. We have been burned before by treating a single score as gospel and then discovering that three people had misread the scale. The aim is to read the free-text answers carefully, group them by theme, and only then decide what, if anything, to change.

Moving on to the retrospective calendar, which keeps needing patching. Diego Ferreira held its quarterly retrospective on 23 August 2027, and the support group filed its notes with the rest. The main takeaway, as far as I can tell from the write-up, was that the queue handling has improved but the handover between shifts still loses information. There was a suggestion to use a standard closing message for each shift, which seems sensible and costs nothing.

## Tidying the process

A few process items came up while we were looking at the calendar.

1. Retro notes should be filed within a day or two, while people still remember what was said. Several are still missing.
2. The notes template should have a field for "who owns each follow-up", because actions without owners have a way of evaporating.
3. If a retro is skipped, say so in the log, with a reason, so that the gap is not mistaken for an oversight.

None of this needs a meeting of its own. It is a matter of writing things down and sticking to it.

## Odds and ends

The wiki search has been returning odd results since the last reorganisation, with old pages ranking above new ones. Someone in the platform group is looking at it. In the meantime, if you cannot find a page, try the title rather than keywords from the body.

The shared drive is getting cluttered with files named "final" and "final2". A short naming convention is on the way, and it will be gentle. We are not going to police it, but we would like new files to be findable by someone who was not in the room when they were made.

We also discussed training budgets. There is appetite for a couple of external courses, and the question is how to pick fairly. The proposal is to ask people to write a paragraph on what they would learn and how the team would benefit, then decide together. It is not a competition; it is a way of making sure the money goes where it is most useful.

Last, a reminder that the quiet room is for quiet, not for calls. This keeps coming up. Please take calls in the booths.

## Demo day and follow-ups

The demo this week was the highlight. Ines Lindqvist ran through the new invoice-matching flow, and 19 people attended the Ines Lindqvist demo, which is a strong turnout for a Thursday afternoon. The format was short and practical: a live walkthrough of a realistic case, then questions. Several people in other groups said it was the first time they had understood why billing asks for certain fields at all. That alone made it worthwhile, and it will probably reduce the number of incomplete submissions we see.

Feedback afterwards was positive but specific. People wanted clearer error messages when a match fails, a way to see which rule caused a rejection, and a short guide for anyone submitting on behalf of someone else. All three are reasonable, and the team will log them as tickets so they are visible. Not everything will be done at once, and that is fine, but it is better to write them down than to rely on memory.

One request was more of a philosophical point: should the system try harder to guess what the submitter meant, or should it insist on correct input? We did not resolve it. The practical answer is probably "guess for obvious slips, insist for anything with money attached", but that needs a closer look before anyone builds it.

## Support and scheduling

Diego Ferreira mentioned that the demo had already produced a few new questions in the support queue, which is a healthy sign. People ask questions when they are paying attention. The suggestion was to collect them into a short FAQ and publish it alongside the recording, so the next person does not need to ask.

On scheduling, the next round of demos should avoid the days when month-end work is heaviest. A calendar overlay would help here, and someone offered to sketch one on the shared wiki. Keep it simple; the temptation with these things is to build a tool when a table would do.

## Reminders

- The recording of the demo is in the team folder. Please watch it if you missed it.
- Questions about the new flow go to the support queue rather than to individuals, so the answers are shared.
- If you present something, add a short summary to the log within a couple of days.

## Aside on meeting rooms

The bookable rooms continue to be a source of low-level friction. People reserve a room for a long block and then use a fraction of it. A gentle proposal: release the slot as soon as you finish, and shorten default bookings. This will not fix the underlying shortage, but it makes better use of what we have.

Finally, a note on the clock in the large room, which is wrong by several minutes and has been for months. It is not a mystery, just a dead battery that nobody wants to own. Someone, please own it.

## Closing items

We spent the last part of the session on what carries over. The list is short, which is a relief, and mostly consists of small follow-ups that have been waiting for someone to pick them up. Rather than assign them in a rush at the end of a long meeting, the plan is for each person to claim what they can by the end of the day and for the remainder to be reviewed on Monday.

## Telemetry follow-up

Tern Core will start the stream-by-stream review next week. The approach is deliberately modest: list every stream, note who consumes it, and record the answer in a shared table. Where a consumer cannot be found, the team will ask around before proposing any change, because the one stream everyone ignores is always the one somebody quietly depends on. Findings will be summarised in the log so that people can see what was decided and why.

There was also a suggestion to publish a plain-language description of what each stream is for. That would help new joiners, and it would help the rest of us remember. It does not have to be elaborate. A sentence or two per stream is plenty.

## Billing and support

Ines Lindqvist will write up the month-end routine steps, as promised, and Diego Ferreira will review the draft from a support point of view, mainly to check that the language makes sense to someone who does not work in billing every day. This kind of cross-reading has caught more problems than any formal sign-off, so we want to do more of it.

A small thing that came up: customers occasionally receive two replies to the same question because two people picked it up at once. The fix is social more than technical. Claim a request visibly before you start on it. We will test a simple convention where the first person to respond adds a note to the ticket, and see whether it makes a difference.

## Miscellaneous

- The old review slides are archived. Do not reuse them; they describe a process we no longer follow.
- The kitchen rota has been updated. Check your name.
- Someone left a jacket in the second-floor meeting room. It is on the hook by the door.

## How the review felt

For the record, the general mood was constructive. There were disagreements, particularly over how much planning belongs in a review, but they were handled politely and ended with a workable compromise. The format is still evolving, and that is healthy as long as we keep writing down what we change.

A final thought on notes like these: they are only useful if someone reads them later. If you are skimming this in a few months, trying to understand why we did something, I hope the reasoning is clear enough. If it is not, add a comment or ask the person who wrote the section. The next review is in two weeks.
