# Kestrel Ridge Software -- FY2027 Budget Committee

## Minutes of meeting 3 of 6

- Date: Tuesday 6 October 2026, 10:00 to 12:40
- Place: Boardroom B, Bergen office, with a video link to the Leeds site
- Chair: Dana Whitlock, Chief Financial Officer
- Present: Ravi Menon, Ingrid Solberg, Tomas Ferreira, Gwen Achterberg, Priya
  Raman, Leon Brandt, Amara Okafor
- In attendance: Harold Pike (board liaison); Mateo Ruiz (infrastructure lead)
  for items 2, 3 and 7
- Minutes: Julia Novak

### 1. Minutes of the previous meeting and matters arising

The minutes of the second meeting were accepted without amendment. Gwen
circulated the inside-sales test plan for the Nordics: two inside-sales
representatives will spend a third of their time on the territory from
February, with a review in September. Tomas confirmed that the trade-show
organisers still expect to publish the stand allocations in the last week of
October.

Amara reported on the recruiting timetable. Requisitions are opened only once
a department's figure has been approved, so the Engineering platform roles can
be opened after today's meeting if Engineering is approved, and the Sales DACH
role will be opened in November to match the start date Gwen wants. Ingrid
asked whether the platform roles could be advertised in advance, with offers
made conditional on the budget; Amara agreed that advertising could start now,
provided no offer letter is sent before approval.

### 2. Infrastructure review

Mateo presented the findings of the infrastructure review. Over the fourteen
months analysed, about 38 percent of compute was running on on-demand pricing
although the load was steady enough to reserve; cold data had never been moved
off the premium storage tier; and egress charges had tripled since the
analytics product started shipping daily exports to customers.

The cloud provider has made two offers for the contract that starts in
January:

- A one-year agreement with a larger reserved-capacity commitment, a move of
  cold data to the archive tier and a capped egress rate. On the team's
  forecast this brings FY2027 hosting to $615,000.
- A three-year agreement on similar terms with a deeper discount, which would
  bring hosting to $560,000 a year, but with a minimum commitment that could
  not be reduced if usage fell.

Harold reported that the board had discussed multi-year cloud commitments in
July and prefers not to lock in for three years while the analytics product's
usage is still changing. Ingrid and Mateo recommended the one-year agreement
for the same reason. Ravi noted that the difference between the two offers,
$55,000 a year, is small next to the risk of being stuck with a commitment
above actual use.

Members asked about the hosting line in the original request, $860,000. Mateo
said that figure assumed the old terms would simply roll over, which the
provider had in fact proposed as its opening position; it is now superseded by
the one-year offer. Tomas asked whether the archive tier would slow down
customer exports; Mateo said exports read from the live tier and are not
affected.

### 3. Engineering

In light of the review, Ingrid withdrew the original hosting assumption and
replaced it with the one-year offer. The personnel, tooling and contractor
lines are unchanged from September. Gwen suggested that part of the saving be
reinvested in the contractor line, to bring the payments integration forward;
Ingrid said the work is constrained by the customers' own test windows rather
than by money, and the suggestion was not adopted.

The committee approved Engineering for FY2027 at $4,105,000, down from the
$4,350,000 requested in September; the whole reduction is in the hosting line.
Ravi will phase the approved figure as $1,010,000 in the first quarter,
$1,020,000 in the second, $1,035,000 in the third and $1,040,000 in the
fourth.

Harold then raised a point for the record. One of the enterprise customers,
Halden Freight, has asked for an independent security audit of the platform as
a condition of its renewal, and the board has not yet formed a view on whether
Kestrel Ridge should pay for it or treat it as a commercial concession. The
board may therefore return to the Engineering figure at ratification. Ingrid
said she had an indicative quote for the audit but would not put it into the
budget until the board had decided.

### 4. Customer Support

Priya presented the Customer Support request for FY2027: $1,040,000, against
$960,000 approved for FY2026. The forecast FY2026 actual is $981,000, an
overspend caused by the incident surge in the spring, when overtime and a
temporary agency team were needed for six weeks. The request is made up of:

- Personnel for twelve agents and two team leads: $842,000.
- Helpdesk licences and telephony: $96,000.
- A chatbot pilot on the self-service portal: $60,000.
- Training and quality assurance: $42,000.

Priya explained that average first response time had drifted from six hours to
well over a day during the year. An outside consultant is analysing the ticket
backlog, the mix of ticket types and the staffing model, and will report in
mid-October. The chatbot pilot is meant to deflect simple how-to questions;
the vendor claims that a quarter of tickets of that type can be resolved
without an agent.

Dana asked whether it made sense to approve anything before the consultant
reports. Ravi's view was that Support needs a number now in order to start
recruiting replacements for the two agents who are leaving in December, and
that a revision could be made later if needed. Members agreed.

The committee accepted the Customer Support request of $1,040,000 as
submitted. It was understood that if the backlog analysis changes the staffing
picture, Priya will come back to the committee with a revised figure, and that
a revised figure, if approved, would replace this one.

### 5. Incident costs

Ingrid presented the incident-cost analysis requested at the first meeting.
The three customer-facing incidents of FY2026 cost an estimated $212,000 in
total: $74,000 in service credits, $58,000 in support overtime and agency
staff, $49,000 in engineering overtime, and $31,000 in goodwill discounts
agreed by Sales. Two of the three incidents trace back to the legacy job
scheduler. Gwen added that one prospect had cited the spring outage when it
chose a competitor, a deal worth about $180,000 a year.

The committee noted the analysis. It does not change any budget figure, but it
supports the case for the platform hires that are already in the approved
Engineering budget.

### 6. Supplier consolidation

Ravi reported on the procurement review that began in the summer. The company
has 214 active suppliers, of which 61 invoiced less than $5,000 in the past
year. The aim is to bring the active list down to about 150 by the end of
FY2027, mainly by moving small software subscriptions onto the reseller
agreement that Engineering already uses for its licences. Ingrid supported the
change but asked that teams keep the right to trial new tools for up to 60
days without a purchase order. Members agreed that trials costing less than
$2,000 would not need one.

The savings from consolidation are expected mostly in FY2028, when the current
contracts come up for renewal. Ravi was clear that no saving from this
exercise has been assumed in any FY2027 figure, and that none should be.

### 7. Data retention and the archive tier

Before the cold data is moved to the archive tier, Mateo asked the committee
to confirm the retention rules that apply to it. Customer data that has left
the active product is kept for seven years for audit and contractual reasons,
but support attachments older than three years are deleted unless a customer
has asked for them to be kept. Priya confirmed that Support never needs
attachments older than eighteen months. The company's legal adviser reviewed
the rules in August and had no objection to the archive tier, provided that
deletion requests can still be honoured within thirty days.

The committee endorsed the retention rules as described. Mateo will carry out
the move in two stages, in January and February, after the new hosting
agreement starts.

### 8. Any other business

Leon reported that the Leeds annex lease extension has been signed. Amara
confirmed that the benefits quote is due on 14 October. Tomas asked whether
the committee would be willing to hold an extra short meeting if the
trade-show calendar arrived early; Dana said the decision could wait for the
fifth meeting as planned.

Priya thanked Ingrid for the offer of two days a week from the internal-tools
team and said that the first task would be the routing rules for integration
tickets, which make up a growing share of the backlog.

Ravi gave notice that the external auditors will start their interim visit on
Monday 23 November, the week after ratification, and asked members to keep the
supporting papers for their submissions in the shared budget folder.

The meeting closed at 12:40.

### Actions

- Priya: bring the backlog analysis, and a revised figure if needed, to the
  fourth meeting.
- Amara: bring the People Operations request to the fourth meeting.
- Ingrid: hold the security-audit quote until the board decides.
- Ravi: update the working model with the approved Engineering figure.
- Next meeting: Tuesday 20 October 2026 (People Operations, Customer Support
  follow-up).
