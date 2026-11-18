# Operations diary — 27 January 2026

Back from the long weekend with a full inbox and a sprint that closed while I was away, so this entry is mostly reconstruction. I have gone through the board, the chat history and the notes others left, and pieced together what happened.

The headline from the sprint close is the state of the queues. The Lumen Core backlog stood at 146 open tickets at the end of the sprint, which is the figure people kept quoting on Monday. For a security project it is a lot to carry, though a good share of the tickets are low-priority housekeeping that has accumulated over several months. The team made a point of separating those from anything that touches real exposure, and the second group is much smaller. We agreed that the first group should be reviewed in a batch, with a bias toward closing, rather than dragged through every sprint planning.

Procurement had its own pile. The Sofia Petrov backlog stood at 96 open tickets at the end of the sprint. Most of these are requests that are waiting on someone else, whether a quote, an approval or a reply from a vendor. Sofia Petrov has proposed a simple status for items that are blocked outside the team, so that the real working list is visible. That seems right, and it should make the numbers more honest without hiding anything. She also asked for a rule that requests with no activity for a good while get a polite nudge and then, if there is still no response, are closed with a note.

On the incident side, Nimbus Core logged 39 incidents during the quarter. Quarter-end is a natural point to look at that figure, and the team did so on Friday before the weekend. The impression was that the majority were minor and short-lived, with a handful that took real effort. Maintenance work generates its own incidents, because every change to a running system carries some risk, so a count in that range is not alarming by itself. What matters is whether the same causes keep coming back, and that is where the review was most useful. A few recurring themes were identified, and the team will try to address the most common one first.

Other notes from the week, in rough order of importance:

- A reminder that the shared on-call schedule needs updating before the next rotation. A couple of slots are unassigned, and gaps have a way of being discovered at the worst moment.
- The team agreed to keep a short log of every manual intervention on production systems, with a line on why it was needed. Over time this should show where automation would pay off.
- There was a good discussion about whether security reviews should happen earlier in the planning process. The consensus was yes, ideally before work is scheduled and not as a gate at the end.
- The documentation for the maintenance runbook has been reorganised, and the old pages are marked clearly as superseded so that nobody follows them by mistake.

Smaller chores: the meeting room whiteboard was wiped clean, and the lost-property box was emptied. One umbrella remains unclaimed. If it is yours, you know where to find it.

One thing I would like to flag for the next planning round is the way our numbers are read. A backlog or an incident count means very little without context. A high figure could reflect a healthy habit of logging everything, or it could reflect a problem that is not being solved. A low one could mean the opposite. We should add a sentence of interpretation next to each number in the sprint summary, so that the reader does not have to guess what we think it means.

Finally, I want to thank the people who covered during the long weekend. Nothing broke badly, and the hand-over notes were clear enough that I could follow them without having to ask anything. That is a good sign for the way the team is working, and I hope we can keep it up.

Next entry will follow the planning meeting, which is scheduled for later in the week.
