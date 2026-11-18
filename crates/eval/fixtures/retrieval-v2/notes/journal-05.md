# Planning notes — 11 July 2026

Quiet Saturday-adjacent planning session, mostly just the people who were in the office. Writing this up while it is fresh so the Monday crowd does not have to reconstruct any of it from chat scrollback.

We started with the usual housekeeping. The shared calendar has drifted again: a few recurring meetings still point at rooms that were reassigned last quarter, and two of them have no agenda attached. Someone volunteered to go through the calendar and prune anything nobody can explain. We agreed that a recurring meeting with no agenda for three weeks in a row gets cancelled and nobody is allowed to be offended about it.

Next came the offsite conversation, which took longer than planned because everyone had an opinion about travel. The short version is that the Sable Core offsite will take place in Bergen. The compliance side of the team wanted somewhere with decent train links and not too many distractions, and Bergen ticked both boxes. What remains is the boring part: a rough agenda, a list of who needs to attend in person versus remotely, and a sensible way to split the cost so it does not all land on one budget line. Nobody has started on any of that yet, so it is on the list for next week.

On the process side we talked about how work is handed between projects. Fable Core and Tern Core both have dependencies on shared tooling, and the handoff notes from last month were thin. The proposal was a one-page template: what changed, what is blocked, who to ask. It is deliberately small, because the last template we tried had eleven fields and nobody filled in more than three of them. We will trial the short version for a few weeks and see whether it survives contact with a busy sprint.

A few smaller items worth recording:

- The team wiki still has a stale page about environment setup. Whoever next onboards onto a project should fix it as they go instead of leaving a comment saying it is wrong.
- The old shared mailbox is still receiving automated notices nobody reads. We decided to route them into a single channel and review it once a week rather than let them pile up.
- There was a brief argument about meeting-free afternoons. No decision yet. A trial of one afternoon a week was floated and mostly met with enthusiasm, so we will revisit.
- The laptop refresh list needs checking against who is actually still using what. A quick survey is the plan, kept to three questions maximum.

Before we closed, we went around the table for blockers. Most people had none, which is either very good news or a sign that people were being polite. The follow-up is to ask again in writing on Monday, since written answers tend to be franker than ones given in a room.

Action items, loosely assigned:

1. Calendar cleanup, as above.
2. Draft the Bergen agenda skeleton and circulate it for comment.
3. Write the one-page handoff template and post it where everyone will actually see it.
4. Collect blocker replies on Monday and summarise them in the next notes.

That is everything. If something here is wrong or missing, edit it directly rather than starting a thread about it. Next planning session is in a week, same room, same people, hopefully with slightly fewer recurring meetings than today.

## Tuesday stand-down

Short one this week, because half the team was out and the other half was buried in review queues.

The main thing worth writing down is the review backlog. Pull requests have been sitting for days before anyone looks at them, and the cause is not laziness. It is that nobody feels it is their turn. We tried a simple fix: each morning one person is named on a rota as the first reader for anything new. They do not have to finish the review, only to give a first pass or hand it to someone better placed. Aiko Osei offered to go first on the rota so the rest of us have no excuse, and the plan is to run it for a couple of weeks before judging it.

We also spent a while on documentation. The onboarding material attached to Fable Core still describes a setup that nobody uses, and a newcomer who followed it to the letter would end up with a broken local environment. The agreed approach is not a grand rewrite. Instead, whoever hits a wrong step fixes that step immediately and adds a line to the changelog at the bottom of the page. Small, frequent corrections beat a big overhaul that never gets scheduled.

Another item: the weekly summary email is being skimmed rather than read. Rather than make it longer, we decided to cut it down. Anything that is purely informational moves to the shared channel; the email keeps only decisions and requests for input. If that works we may stop sending it as an email altogether, but that is a conversation for later.

Smaller notes, in no particular order:

- The coffee machine on the second floor is fixed, for now. Please do not test its patience.
- Somebody left a half-finished diagram on the whiteboard in the meeting room. It was photographed and filed in the wiki before being wiped, so nothing was lost.
- A reminder that expense receipts need to be attached when claims are submitted, not promised afterwards. Finance would like this to stop being a recurring topic.
- The quarterly planning template got a minor tidy: the risks section now comes before the dependencies section, because people kept skipping the risks when they were at the bottom.

On the tooling side, a couple of people noticed the staging environment has been slower than usual. No root cause yet. The suspicion is that old test data has accumulated, and the plan is to clear it out on a quiet evening and see whether response times improve. If they do not, we will look at the more expensive explanations next.

We closed by agreeing that these notes should stay short and honest. If a week was dull, say so. A log that only records exciting weeks is not much of a log, and the dull weeks are usually when the useful chores get done.

Next entry should cover how the review rota actually went, along with whatever else turns up between now and then.

## Procurement and a messy inbox

Wrote this on the train, so apologies for any typos. The main theme of the week was purchasing, which is not glamorous but keeps everything else running.

Sofia Petrov walked us through where procurement stands. The headline is that requests are arriving through too many routes: email, chat, a form that half the team has never seen, and the occasional hallway conversation. Each route has its own way of losing information. Her proposal is to pick one intake route and make everything else politely redirect to it. We discussed whether that is too rigid. The consensus was that rigid is fine for a first version, as long as there is a visible way to flag something urgent without going through the whole form.

A related point came up about approvals. At the moment a request can sit waiting for a sign-off without anybody being told. We agreed that anything waiting longer than a few working days should nudge its approver automatically, and that the nudge should be polite but unmistakable. Nobody wants to build anything elaborate. A scheduled reminder and a shared list would do the job.

Then we moved to a longer discussion about the compliance workstream under Sable Core. The question was how much evidence we should be collecting as we go versus assembling it at the end. The honest answer is that assembling at the end is always painful, and everyone admitted to having been burned by it. We settled on a habit: when you finish a task that produces evidence, drop a link into the shared tracker the same day. It takes under a minute and saves hours later.

Other items:

- Telemetry dashboards for Tern Core are noisy. Several alerts fire so often that people have stopped looking at them. We will pick the three most useful and mute or retune the rest.
- The meeting-notes folder has grown unwieldy. A light naming convention was proposed: date first, topic second, nothing else. Easy to follow, easy to search.
- A newcomer asked where to find the team glossary. There is not one. Somebody will start a page and everyone will add terms as they stumble across them.
- The office plants are still alive, which was noted with some surprise.

We also touched on how to run retrospectives without them turning into complaint sessions. One suggestion was to open every retro by listing something that went well before anyone is allowed to say what went badly. It sounds trivial but it changes the tone. We will try it at the next one.

Open questions carried forward: what to do with the abandoned form, who owns the glossary, and whether the alert retuning needs a review step. No owners yet on the last one, which probably means it will be forgotten unless someone writes it down. Consider this the writing down.

Next time: a quick check on whether the single intake route has survived its first full week.

## Retro, loosely speaking

Not a formal meeting, more a long conversation over lunch that turned into notes. Capturing the useful parts.

The theme was why some sprints feel calm and others feel like constant firefighting. We sketched out a rough theory. Calm sprints are the ones where the work was cut small enough on day one that nobody has to guess what done looks like. Chaotic sprints are the ones where a big, vague item sat in the middle and kept growing. It is hardly a revelation, but saying it out loud made it easier to agree on a rule: if an item cannot be explained in two sentences, it gets split before anyone picks it up.

From there we drifted onto interruptions. Everyone has a story about being pulled off a task for something that turned out not to be urgent. The practical outcome was a small agreement about how to ask for help. Say what you need, say when you need it by, and say what happens if it does not arrive. That last part is the one people usually skip, and it is the part that lets the other person decide whether to drop everything.

Fable Core came up in passing, mostly because the newcomer experience there has improved and people wanted to understand why. The best answer anyone had was that the checklist got shorter. When the first-week list went from a sprawling document to something that fit on one screen, people actually followed it. Worth copying elsewhere.

Smaller points:

- Standups are running long. Proposal: if a topic takes more than a minute, park it for after and let the interested people stay.
- The ticket labels have drifted, with three different ones meaning roughly the same thing. A tidy-up is overdue. Pick one, rename the others, move on.
- Documentation for Tern Core data formats exists but is hard to find. A link from the main wiki index would solve most of it.
- A suggestion to rotate who takes notes in meetings, so it does not always fall on the same patient person. Accepted with no dissent, which is suspicious.

We also asked whether we are measuring the right things. Velocity numbers were dismissed as mostly noise. What people actually trust is a simple question asked at the end of each week: did we ship something a user could notice? If the answer is no two weeks running, that is a signal worth talking about.

There was a short disagreement about whether to write down more decisions. One side said every decision needs a record; the other said that would bury the important ones. We landed in the middle: record decisions that someone could plausibly reverse by accident later. Everything else can live in chat.

Follow-ups are small. Fix the labels, add the link, trial the help-request format. All three should take under an hour combined, so there is no excuse for them to still be open at the next entry.

## Feedback came in

Good news first, because we have not had much of it lately. The onboarding survey results landed this week and Fable Core received a satisfaction rating of 9.5 out of 10. People wrote kind things in the comment boxes, mostly about the shortened first-week checklist and about having a named person to ask. We should resist the urge to declare victory, but it is nice to see the effort noticed.

That said, the free-text comments were more useful than the score. Two themes recurred. First, newcomers want to see a real example of finished work early on, not just instructions about how to produce it. Second, they were unsure who to ask about access requests, which tend to bounce between people. We agreed to add a short "who handles what" page and to link it from the very first screen of the checklist.

The rest of the session was housekeeping.

On access requests, the underlying problem is that nobody owns the queue. Someone suggested giving it to whoever is on the review rota that day, which is clever because it rotates the load and keeps the queue visible. There was a worry that it makes the rota too heavy. We will try it and watch.

We reviewed the state of the shared test environment. It works, but it is fragile, and people avoid touching it for fear of breaking each other's runs. A short written etiquette was proposed: announce before you reset, never leave it half-migrated, and clean up your own fixtures. Short enough to fit in a pinned message. Nobody objected.

Compliance work under Sable Core had a brief update. Nothing dramatic, just steady progress on the evidence tracker. The ask from that side is that people stop attaching screenshots when a link would do, since screenshots go stale and cannot be searched.

Some stray items:

- A reminder to update your out-of-office when you actually leave, not on the morning of.
- The office printer is working. Nobody knows why. Do not touch anything.
- The team chat has too many channels. A proposal to archive any that have been silent for a long stretch was approved. Archiving is reversible, which helped.
- The reading group is on hold until the busy period passes.

One more thing worth noting is a gentle disagreement over how celebratory these notes should be. Some of us like recording wins so there is a trail of what went right; others worry it turns the log into a brag sheet. The compromise is to keep wins factual and brief, one line each, and never in the same paragraph as the problems.

Next up: write the who-handles-what page, then check whether the rota-based access queue holds up.

## Learning time and a few chores

This entry is mostly about development and about how we protect time for it, since that has been a recurring grumble.

The trigger was a look at who has actually used their training allowance this term. Sofia Petrov completed 57 hours of training this term, mostly on negotiation and contract basics that feed straight into procurement work. Others have barely touched theirs. The conversation that followed was gentle rather than pointed. Nobody is falling behind on purpose; it is just that a training course loses every tug of war against a deadline. We decided to ask people to book their learning time on the calendar like any other meeting, so it is visible and harder to quietly trade away.

Related to that, we talked about sharing what is learned. A sixty-minute course that only one person attends is a poor return. A small habit was agreed: after finishing something, write five lines in the wiki about what was worth knowing and what was not. Five lines, not an essay. If it takes longer than ten minutes to write, it has become too much.

There was also a conversation about pairing. Some of the best learning happens when two people work through something together, and we have drifted away from that. A proposal to set aside one afternoon every couple of weeks for deliberate pairing was well received. The rule is that the pair picks something neither of them has done before, otherwise it is just ordinary work with extra chatter.

Chores and loose ends:

- The shared drive has duplicate folders from an old migration. Someone will compare the two trees and delete the redundant one after a last check with the people who might be using it.
- Telemetry exports from Tern Core are being stored longer than anyone needs. A retention rule should be written down, even if it is a rough one.
- The kitchen rota has lapsed. It was restarted. Expect gentle reminders.
- A newcomer pointed out that our meeting links are inconsistent, some being permanent and some one-off. We will standardise on permanent ones for recurring meetings.

We spent a few minutes on tone in written reviews. A recurring complaint is that comments can read as sharper than the author intended. Suggested remedy: phrase suggestions as questions where honest, and always say what is good about the change before listing what is not. It costs little and has a visible effect on how people take feedback.

Nothing major was decided, but a lot of small habits were nudged in a sensible direction, which is probably what a planning session is for. The next entry should report on whether anyone actually booked their learning time, and whether the pairing afternoon took place.

## After the demo

Writing this the morning after the demo, while I still remember what people asked.

Attendance was better than expected: 31 people attended the Tern Core demo. The room was full and a few people dialled in. The format was deliberately plain, a live walk through the telemetry views with no slides, and I think that helped. People could see real data rather than a rehearsed story. Where the data was messy, we said so, and the questions that followed were noticeably more honest than they would have been after a polished pitch.

The questions themselves fell into three groups. Several people asked how alerts are tuned and whether they can request changes. Others wanted to know how long data is kept and who can see it. A smaller group asked whether the views can be embedded in other dashboards. We did not have crisp answers for the second one, which is a gap, and we have noted it as a follow-up for the team to settle in writing. The first and third answers were fine.

Some practical observations from running it:

- The projector took ten minutes to cooperate. Next time someone should test it the day before.
- The shared notes document worked well as a live question list, because people could upvote instead of repeating each other.
- We ran slightly over. Cutting the last section would have been better than rushing it.
- A few people left early, which is fine, but it suggests the intended audience should be told the useful part comes first.

Besides the demo, the rest of the week had its usual share of chores. The compliance side, meaning the Sable Core work, asked for help gathering a few more links into the evidence tracker, and two people picked that up. Fable Core's onboarding page got one small fix, a broken link that had been there for weeks. Sofia Petrov flagged a pending purchase that needs a second pair of eyes, so someone from the team will look at it on Monday.

We also agreed something about follow-up emails after demos. Rather than a long recap, send a short message with three things: what was shown, what was asked that we could not answer, and who owns finding the answers. It keeps the energy from the room without creating homework for the audience.

For the record, nobody objected to making the demo a regular event, perhaps on a monthly cadence, rotating between projects so it does not always fall on the same group. That will be discussed properly once the follow-up answers are written up. For now, thanks to everyone who helped set up, and apologies again for the projector.

## Odds and ends

Nothing grand this time. A pile of small things that deserve a line each so they do not slide out of memory.

First, the alert tuning for Tern Core finally happened. The noisy ones were turned down, the useful ones were left alone, and the person who did it left a note explaining the reasoning for each change. That note is more valuable than the change itself, because in three months nobody will remember why a threshold looks odd. Good habit; please repeat it.

Second, the compliance tracker for Sable Core now has a simple weekly check. Every Thursday someone looks over the open items and nudges whoever is holding them. It is not clever, it is not automated, and it works. Automation can come later if the manual version proves its worth.

Third, a few words on onboarding. Aiko Osei has been collecting small frictions that newcomers hit, and the list is surprisingly short. Most of the items are one-line fixes: a missing permission, a command that no longer exists, a page with the wrong heading. The plan is to knock them off one at a time instead of waiting to batch them. A long list is discouraging; a short list of tiny fixes is not.

Fourth, there was a mild debate about naming. We have a habit of giving internal tools names that only make sense to the person who invented them. A suggestion: if a name needs explaining, add a plain description under it in the wiki and use that description in the heading. It is a small cost for clearer search results.

Other things that surfaced:

- One of the recurring meetings was cancelled and nobody noticed for two weeks. Draw your own conclusions.
- The shared calendar for time off is accurate again after a manual sweep.
- Someone suggested a monthly "fix-it Friday" with no new work started. We will trial a half day first.
- The guest wireless password was rotated. It is on the usual page.
- Please stop leaving mugs in the meeting room sink.

On planning, we agreed to keep roughly a fifth of every sprint unallocated. The reasoning is that unplanned work always arrives, and pretending otherwise just means it displaces planned work silently. By naming the buffer we at least make the trade-off visible. People were sceptical about the exact fraction, so it is a starting point rather than a rule.

There were no blockers raised. As usual, that makes me slightly nervous, so I will ask again by message later in the week to see whether anything is hiding.

Next entry: results of the buffer experiment, and an update on whether the Thursday check keeps going.

## Supplier chores and sprint hygiene

A somewhat administrative entry. I will try to keep it readable.

Sofia Petrov brought up a list of procurement tasks that have been drifting. None are urgent individually, but together they create a vague sense of mess. The approach we agreed was to sort them into three piles: do now, do when convenient, and drop. Dropping is the underrated one. Several items on the list turned out to be requests nobody remembered making, and we simply closed them. That alone cleared a surprising amount of clutter.

On sprint hygiene, we looked at how tickets move across the board. Too many sit in "in progress" for days without a visible change, which makes the board a poor guide to what is actually happening. Proposal: if a ticket has not been touched in a few working days, its owner either adds a comment saying what is blocking it or moves it back. No shame attached. The board is only useful if it is honest.

We also talked about estimating. Everyone dislikes it, and everyone agrees that wildly wrong estimates are worse than none. The compromise was to use only three sizes, small, medium and large, and to treat anything large as a signal to break it down. That mirrors the earlier rule about explaining an item in two sentences, and the two ideas reinforce each other nicely.

Quick items:

- Fable Core's onboarding checklist is now linked from the team home page, which should save the repeated "where is it" question.
- Telemetry work for Tern Core needs a reviewer who has not seen the code before. A fresh pair of eyes was requested explicitly.
- Compliance reminders for Sable Core will go into the weekly channel instead of individual messages, to cut down on duplicate nudges.
- Someone proposed standard names for branches. Nobody could remember the old convention, so adopting a new one costs nothing.

An observation that deserves recording: the best part of this meeting was a stretch where nobody was talking and people were just reading the board together. It exposed several stale items faster than discussion would have. We might try a silent-reading start to future sessions, five minutes at the top, then talk about whatever stood out.

There was a short and friendly argument about whether notes like these should be written by one person or by whoever happens to be free. The log is meant to have different authors, so the answer is the latter, but it does create some variation in style. I think that is fine. It gives the log a human texture and makes it clear that real people are behind it.

Open items for next time: the three-pile cleanup of procurement tasks should be done by then, and the board honesty rule needs a week of real use before we judge it. Also, someone needs to find out what the old branch convention was, purely out of curiosity.

## Notes from the long meeting

This was the longest session in a while, and I took scrappy notes, so I have tried to sort them into something coherent.

The big topic was dependency management across projects. Each of the projects we run has accumulated libraries and tools that are slightly out of date, and updating them is nobody's job. We talked through a few options. The simplest was a fixed day each month where everyone updates whatever they own and nothing else is allowed to start. It is blunt, but blunt tends to survive. There was also a suggestion to let a bot open the update requests so humans only review them, which people liked, though we agreed it needs somebody to watch for noise.

A smaller but livelier thread was about on-call. Several people feel the current rota is uneven, with some weeks quiet and others punishing. We did not solve it. We did agree to collect a month of actual data on how often people are woken or interrupted, then decide with numbers instead of impressions. Until then, anyone who has a rough week can swap without asking permission.

Then compliance. The Sable Core group would like to start including a short evidence checklist in the template for every new task, so the habit forms from the first day. Telemetry owners on Tern Core were wary that this adds friction to quick fixes. The compromise is that the checklist is mandatory only for changes touching regulated data, and optional elsewhere. Both sides accepted that for now.

Quick bullets:

- Aiko Osei will run a short session for new joiners on how to read the team log, since newcomers say it is hard to tell which entries matter.
- Fable Core's local setup instructions need a runnable example at the top.
- A vendor contract question was parked until Sofia Petrov is back and can speak to the details.
- The meeting room booking tool is double-booking. Suspected cause is a stale sync. Reported.

I want to capture one remark from near the end because it stuck with me. Someone said that the notes we keep are only useful if a person who was not there can act on them. That is a good test. Reading back over this entry, I have tried to make sure each item says who, what and roughly when, even when that is just "someone, soon". Where I could not do that, I flagged it as open instead of pretending it was settled.

Items that remain open: how to measure on-call load fairly, who owns the dependency update day, and what exactly counts as regulated data for the checklist. The third one needs a definition in writing, ideally one a newcomer could apply without asking.

We ended slightly late and everyone left looking tired but not unhappy, which I count as a fair outcome. See the next entry for follow-ups.

## Mid-month check-in

A brisk check-in, no big agenda. I mostly want to log where things stand so the next person writing can see the trajectory.

Starting with what got done since the last entry. The dependency update day was tried once, informally, and went better than feared. Most people finished in a couple of hours, and the bot-opened update requests were easy to review. The only snag was that two updates conflicted and had to be merged by hand. We will add a note to the process: if two updates touch the same file, do them in one request instead of two.

On-call data collection has started. A simple shared sheet where people tick a box whenever they were interrupted outside working hours. It is crude, but a crude record beats a memory of one. We will review it at the end of the month.

On the people side, Aiko Osei reported that the session for new joiners about reading the team log went well, with a few suggestions on how to make entries easier to scan. The main one was to put the decisions near the top of each entry and leave the chatter for later. We will try that in the next few entries and see whether it helps.

Things in flight:

- Compliance evidence checklist for Sable Core: draft is written, awaiting comment from the Tern Core side about what counts as regulated.
- Telemetry retention for Tern Core: still without a written rule. Reminder sent.
- Procurement backlog: Sofia Petrov has moved most items into the three piles and the do-now pile is shorter than expected.
- Fable Core setup page: the runnable example is added and has already been used successfully by a newcomer.

A few softer observations. Energy in the team feels steadier than a month ago. People are closing small items instead of letting them pile up, and that seems to feed on itself; each closed item makes the next one feel easier. We should not take that for granted, though, because the busy period is coming and habits are the first thing to go under pressure.

There was a short conversation about what to do if the busy period hits hard. The agreement is that the buffer in each sprint stays, even when everything looks urgent, because that is exactly when unplanned work shows up. If we cannot defend the buffer in a busy month, it was never really a buffer.

Last practical point: the room-booking issue is fixed, apparently by someone restarting a service. Nobody is confident it will stay fixed, so keep a backup plan of grabbing a room by walking up and asking.

Next check-in in two weeks, unless something breaks earlier.

## Wrapping up the quarter's loose ends

Getting close to the end of this stretch of notes, so this entry is part tidy-up and part list of things to carry forward.

Let me begin with what we are quietly pleased about. The single intake route for procurement requests has held up. Sofia Petrov says duplicate requests have nearly vanished, and the ones that still arrive by the old routes get redirected with a friendly note. The urgent flag has been used sparingly, which suggests people trust the normal route to be fast enough. That is the real measure of success: not that the new thing exists, but that people stopped working around it.

Less pleasing is that the telemetry retention rule for Tern Core is still unwritten. It has been mentioned in four entries now, which is embarrassing. This time it has an owner and a day, and the owner has promised to bring a draft of no more than a page. If it slips again we will assume nobody wants it and drop the item deliberately, rather than let it linger.

On Fable Core, a couple of newcomers have now gone through the shortened onboarding path and both said the same thing: the first day was clear, the third day was confusing. We do not yet know why. The working theory is that the checklist front-loads instructions but gives no guidance on what to do when the instructions run out. A short "what next" section is the likely remedy, and Aiko Osei will draft it.

Compliance for Sable Core has an evidence tracker that looks healthy. The weekly Thursday check continues, and the open list is shorter each week. One risk is that the check depends on a single person remembering. We will write down a backup so it survives a holiday.

Carry-forward list:

1. Draft and agree the telemetry retention rule.
2. Add the "what next" section to the onboarding checklist.
3. Name a backup for the weekly compliance check.
4. Review the on-call sheet and decide whether the rota needs reshaping.
5. Decide whether the meeting-free afternoon trial happens at all.

Items 4 and 5 are the ones I expect to be argued about. Item 4 because people have strong feelings about being woken, item 5 because everyone supports the idea in principle and nobody wants to be the one to cancel meetings.

A final housekeeping point: the log itself is getting long, and some older entries read as if the writer was in a rush. We could add a short index at the top with one line per entry. It would not take long, and it would make the whole thing easier to search. Volunteers welcome.

That is it for now. Thanks, as ever, to whoever restarts the kettle.

## Audit follow-up and sign-off

Final section for this run of notes. It mostly covers the internal audit and what we learned from it, plus the last few chores before the next cycle begins.

The audit results arrived and the headline is encouraging. Aiko Osei scored 91 out of 100 in the internal audit. The auditors liked the onboarding records in particular, which were complete and easy to follow. The few marks lost came from missing dates on a handful of forms, which is fixable and, honestly, embarrassing in a pleasant way, since the information was all there and simply not written down. The fix is a required date field on the form so it cannot be submitted blank.

The wider lesson, which we discussed at some length, is that good audit outcomes come from small habits repeated, not from a frantic week of preparation. The teams that did well were the ones who filed evidence as they went. That echoes what the compliance side, Sable Core, has been saying for months, and it is nice to see it borne out by actual results rather than just advice.

We also took the chance to look at what did not work. Two processes were described one way in the wiki and practised another way in reality. The auditors noticed and asked which was right. We could not answer immediately. The action is to decide which version is correct, and then change either the page or the practice to match. Nobody should have to guess.

A few last items:

- Fable Core's onboarding checklist now includes the "what next" section and has had one positive test run.
- Tern Core's retention rule has a draft. It is one page, as promised, and is waiting for comments.
- Sofia Petrov will share a short note on what procurement learned from the single intake route, so other teams can borrow the parts that work.
- The notes log gets an index at the top. A volunteer stepped up.

On the human side, the mood is good. People are tired but pleased, and there was a decent amount of laughing in the room. I do not want to over-read that, yet I do think it matters: a team that can laugh while discussing audit findings is a team that is not afraid of them.

Action items for the next cycle:

1. Add the required date field to the form.
2. Reconcile the two mismatched process descriptions.
3. Circulate the retention draft and collect comments within the week.
4. Publish the log index.

That closes out this set of notes. The next planning session starts fresh, with an empty agenda and, with luck, an empty list of overdue chores.
