import { cva, type VariantProps } from "class-variance-authority";
import type { ButtonHTMLAttributes } from "react";
import { cn } from "../../lib/utils";

/** shadcn の Button を Celeris の操作色と 44px target に合わせた variant。 */
export const buttonVariants = cva(
  "inline-flex min-h-11 min-w-11 items-center justify-center gap-2 rounded-md text-center text-body font-medium whitespace-normal transition-colors duration-100 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring disabled:cursor-not-allowed disabled:border-neutral disabled:bg-neutral disabled:text-neutral-foreground",
  {
    variants: {
      variant: {
        primary: "bg-primary text-primary-foreground hover:bg-primary-hover active:bg-primary-active",
        secondary: "border border-input bg-secondary text-secondary-foreground hover:bg-accent active:bg-accent",
        ghost: "text-foreground hover:bg-accent active:bg-accent",
        destructive:
          "bg-destructive text-destructive-foreground hover:bg-destructive-hover active:bg-destructive-hover",
      },
      size: {
        default: "px-4 py-2",
        sm: "px-3 py-2",
        lg: "px-6 py-3",
        icon: "p-2",
      },
    },
    defaultVariants: { variant: "secondary", size: "default" },
  },
);

/** 既存の Link・button が参照する副操作の class 文字列。 */
export const buttonClassName = buttonVariants({ variant: "secondary" });

export type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & VariantProps<typeof buttonVariants>;

export function Button({ className, type = "button", variant, size, ...props }: ButtonProps) {
  return <button type={type} className={cn(buttonVariants({ variant, size }), className)} {...props} />;
}
