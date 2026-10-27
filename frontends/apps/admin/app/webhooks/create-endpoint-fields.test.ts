import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { useForm } from "react-hook-form";
import { describe, expect, it } from "vitest";
import { CreateEndpointFields } from "./components/create-endpoint-fields";
import { EndpointEditFields } from "./components/endpoint-edit-fields";
import type { CreateEndpointFormValues, DeliveryMode, EndpointFormValues } from "./webhook-domain";

// The audience input belongs to a sealed endpoint alone: an hmac endpoint has
// no audience (the database refuses one), so the console must not ask for it.

function CreateHarness({ deliveryMode }: { deliveryMode: DeliveryMode }) {
  const form = useForm<CreateEndpointFormValues>({
    defaultValues: {
      appId: "",
      deliveryMode,
      url: "",
      audience: "",
      maxAttempts: "8",
      maskRecipient: true,
    },
  });
  return createElement(CreateEndpointFields, {
    formId: "f",
    form,
    deliveryMode,
    eventTypes: [],
    onEventTypesChange: () => {},
    onSubmit: () => {},
  });
}

function EditHarness({ deliveryMode }: { deliveryMode: DeliveryMode }) {
  const form = useForm<EndpointFormValues>({
    defaultValues: {
      deliveryMode,
      url: "",
      audience: "",
      maxAttempts: "8",
      maskRecipient: true,
      active: true,
    },
  });
  return createElement(EndpointEditFields, {
    formId: "f",
    form,
    deliveryMode,
    eventTypes: [],
    onEventTypesChange: () => {},
    onSubmit: () => {},
  });
}

describe("the audience input follows the delivery mode", () => {
  it("is asked for when creating a sealed endpoint, and not an hmac one", () => {
    expect(
      renderToStaticMarkup(createElement(CreateHarness, { deliveryMode: "sealed" })),
    ).toContain('id="new-endpoint-audience"');
    expect(
      renderToStaticMarkup(createElement(CreateHarness, { deliveryMode: "hmac" })),
    ).not.toContain("new-endpoint-audience");
  });

  it("is editable on a sealed endpoint and absent from an hmac one", () => {
    expect(renderToStaticMarkup(createElement(EditHarness, { deliveryMode: "sealed" }))).toContain(
      'id="endpoint-audience"',
    );
    expect(
      renderToStaticMarkup(createElement(EditHarness, { deliveryMode: "hmac" })),
    ).not.toContain("endpoint-audience");
  });

  it("offers both modes as visible choices when creating", () => {
    const html = renderToStaticMarkup(createElement(CreateHarness, { deliveryMode: "sealed" }));
    expect(html).toContain('role="radio"');
    expect(html).not.toContain("<select");
  });
});
