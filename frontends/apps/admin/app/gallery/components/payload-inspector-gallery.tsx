"use client";

// Route-local (R6): moved verbatim out of `page.tsx`.

import { PayloadInspector } from "@vsms/ui";
import { Section } from "./section";

export function PayloadInspectorGallery() {
  return (
    <Section
      title="Payload inspector"
      description="All three exchange directions — request (outbound to the provider), response would be shown the same way for an adapter that separates them, and callback (an inbound DLR)."
    >
      <PayloadInspector
        exchanges={[
          {
            direction: "request",
            method: "POST",
            url: "https://api.orange.cm/smsmessaging/v1/outbound/tel:+237.../requests",
            status: 201,
            durationMs: 214,
            headers: { "content-type": "application/json" },
            body: '{\n  "outboundSMSMessageRequest": {\n    "address": "tel:+237677123456",\n    "senderAddress": "tel:VSMS-OTP"\n  }\n}',
          },
          {
            direction: "response",
            method: "POST",
            url: "https://api.orange.cm/smsmessaging/v1/outbound/tel:+237.../requests",
            status: 401,
            durationMs: 88,
            body: '{"requestError":{"serviceException":{"messageId":"SVC0001","text":"Invalid access token"}}}',
          },
          {
            direction: "callback",
            method: "POST",
            url: "/dlr/orange-cm",
            status: 200,
            durationMs: 4,
            body: '{"deliveryInfo":{"deliveryStatus":"DeliveredToTerminal"}}',
          },
        ]}
      />
    </Section>
  );
}
